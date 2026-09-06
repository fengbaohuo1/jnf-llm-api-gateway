//! HNSW（Hierarchical Navigable Small World）近似最近邻索引——自包含实现。
//!
//! 算法参考 Malkov & Yashunin《Efficient and robust approximate nearest neighbor
//! search using Hierarchical Navigable Small World graphs》(2016/2018)：
//! - **多层可导航图**：高层稀疏"长程边"快速接近目标区域，底层稠密保证精度；
//! - **随机层级**：新节点层数 `l = floor(−ln(U)·mL)`，mL = 1/ln(M)，保证连通性；
//! - **插入**：入口点在其可达的每一层（0..=l）做 beam 搜索（ef_construction），
//!   取最近 M 个双向连边（超限按"距自身最近"截断）；层数高于入口则升级入口；
//! - **查询**：从入口逐层贪心下降至 0 层，0 层 beam 搜索（ef）后取 top-k；
//! - **距离**：1 − 内积（向量须 L2 归一化，等价余弦距离）。
//!
//! 取舍说明（教学实现）：
//! - 距离为朴素 O(dim) 循环（编译器可向量化）；万级 × 千维构建在秒~数十秒级，
//!   由 kb_tasks 任务化承载；优化方向（分块/量化/多线程构建）见 AGENTS 规划；
//! - 邻居选择用"最近 M 个"简单启发式（未做多样化启发式，召回略低但直观）；
//! - 不支持单点删除：文档增删后整索引重建（与 kb 的"可重建缓存"模型一致）；
//! - 本模块无锁，并发由上层 `VectorIndex` 的 Mutex 串行化（写入任务独占、
//!   查询短临界区）。
//!
//! 正确性说明：高层贪心下降仅为查询加速，任意点从入口经 0 层 beam 仍可达
//! （所有节点都有 0 层边），故低层结果正确，高层只影响召回/延迟权衡。

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// 图节点：一个分块（id=chunk rowid）及其分层邻接表
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Node {
    id: i64,
    /// 向量（L2 归一化）
    vector: Vec<f32>,
    /// 所在最高层
    level: usize,
    /// neighbors[l] = 第 l 层邻居（节点下标）
    neighbors: Vec<Vec<u32>>,
}

/// HNSW 索引（全状态可序列化 → save/load 即整体持久化）
#[derive(Debug, Serialize, Deserialize)]
pub struct HnswIndex {
    dim: usize,
    /// 每层最大连接数（M）
    m: usize,
    /// 插入 beam 宽度
    ef_construction: usize,
    /// 层缩放 mL = 1/ln(M)
    level_mult: f64,
    nodes: Vec<Node>,
    /// 入口点
    entry: Option<u32>,
    /// splitmix64 随机源（确定性可复现）
    rng_state: u64,
}

const M_DEFAULT: usize = 16;
const EF_CONSTRUCTION_DEFAULT: usize = 200;

impl HnswIndex {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            m: M_DEFAULT,
            ef_construction: EF_CONSTRUCTION_DEFAULT,
            level_mult: 1.0 / (M_DEFAULT as f64).ln(),
            nodes: Vec::new(),
            entry: None,
            rng_state: 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 供索引管理命令/状态展示使用（当前检索路径按 entry 判断空）
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 插入（幂等：同 id 忽略，重复构建安全）
    pub fn add(&mut self, id: i64, vector: &[f32]) {
        debug_assert_eq!(vector.len(), self.dim, "向量维度与索引不一致");
        if self.nodes.iter().any(|n| n.id == id) {
            return;
        }
        let idx = self.nodes.len() as u32;
        let level = self.random_level();
        self.nodes.push(Node {
            id,
            vector: vector.to_vec(),
            level,
            neighbors: vec![Vec::new(); level + 1],
        });

        let Some(entry) = self.entry else {
            self.entry = Some(idx);
            return;
        };
        let entry_level = self.nodes[entry as usize].level;

        // 入口能到达的层（≤ entry_level）逐层 beam + 双向连边，由高到低
        for lvl in (0..=level.min(entry_level)).rev() {
            let q = &self.nodes[idx as usize].vector;
            let start_d = distance(&self.nodes[entry as usize].vector, q);
            let hits = self.beam_search_level(lvl, q, &[(entry, start_d)], self.ef_construction);
            let selected: Vec<u32> = hits
                .iter()
                .take(self.m)
                .map(|(n, _)| *n)
                .filter(|n| *n != idx)
                .collect();
            if selected.is_empty() {
                continue;
            }
            self.connect_bidirectional(idx, &selected, lvl);
        }
        // 层数高于入口 → 新点成为入口
        if level > entry_level {
            self.entry = Some(idx);
        }
    }

    /// ANN 检索：从入口逐层贪心下降到 0 层，再 0 层 beam（ef）取 top-k
    pub fn search(&self, query: &[f32], top_k: usize, ef: usize) -> Vec<(i64, f32)> {
        let Some(entry) = self.entry else { return Vec::new() };
        let top_level = self.nodes[entry as usize].level;
        let mut cur = entry;
        // 高层贪心下降（每层收敛到局部最近，作为下层起点）
        for lvl in (1..=top_level).rev() {
            cur = self.greedy_hop(query, cur, lvl);
        }
        let d = distance(&self.nodes[cur as usize].vector, query);
        let hits = self.beam_search_level(0, query, &[(cur, d)], ef.max(top_k));
        hits.into_iter()
            .take(top_k)
            .map(|(n, dist)| (self.nodes[n as usize].id, dist))
            .collect()
    }

    // ---- 算法内部 ----

    /// 随机层级（splitmix64，确定性）
    fn random_level(&mut self) -> usize {
        self.rng_state = self.rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let u = (z >> 11) as f64 / (1u64 << 53) as f64; // (0,1)
        let lvl = (-u.ln() * self.level_mult).floor() as usize;
        // 上限保护：确保 neighbors 分配不失控（约 23 层内）
        lvl.min(32)
    }

    /// 单层贪心：从 cur 出发沿 level 层边走到局部最近点（visited 防环）
    fn greedy_hop(&self, query: &[f32], start: u32, level: usize) -> u32 {
        let mut cur = start;
        let mut visited = std::collections::HashSet::new();
        visited.insert(cur);
        loop {
            let mut improved = false;
            let mut best = cur;
            let mut best_d = distance(&self.nodes[cur as usize].vector, query);
            let ns = self.nodes[cur as usize]
                .neighbors
                .get(level)
                .cloned()
                .unwrap_or_default();
            for n in ns {
                if visited.insert(n) {
                    let d = distance(&self.nodes[n as usize].vector, query);
                    if d < best_d {
                        best = n;
                        best_d = d;
                        improved = true;
                    }
                }
            }
            if !improved {
                return cur;
            }
            cur = best;
        }
    }

    /// 指定层 beam 搜索：以 starts 为种子扩展，保留最近 ef 个（升序返回）
    fn beam_search_level(
        &self,
        level: usize,
        query: &[f32],
        starts: &[(u32, f32)],
        ef: usize,
    ) -> Vec<(u32, f32)> {
        use std::collections::{BinaryHeap, HashSet};
        use std::cmp::Reverse;

        // candidates：最近优先的待扩展池（Reverse 最小堆）
        // results：当前最近集（最大堆，peek 即最差，超 ef 淘汰它）
        let mut candidates: BinaryHeap<Reverse<(Dist, u32)>> = BinaryHeap::new();
        let mut results: BinaryHeap<(Dist, u32)> = BinaryHeap::new();
        let mut visited: HashSet<u32> = HashSet::new();

        for (n, d) in starts {
            if visited.insert(*n) {
                candidates.push(Reverse((Dist(*d), *n)));
                results.push((Dist(*d), *n));
            }
        }
        while results.len() > ef {
            results.pop();
        }

        while let Some(Reverse((Dist(dist), node))) = candidates.pop() {
            let worst = results.peek().map(|(Dist(w), _)| *w);
            if worst.is_some_and(|w| dist > w) {
                break; // 最近候选都比当前最差结果远 → beam 收敛
            }
            let ns = self.nodes[node as usize]
                .neighbors
                .get(level)
                .cloned()
                .unwrap_or_default();
            for n in ns {
                if !visited.insert(n) {
                    continue;
                }
                let d = distance(&self.nodes[n as usize].vector, query);
                let accept = results.len() < ef
                    || results.peek().is_none_or(|(Dist(w), _)| d < *w);
                if accept {
                    candidates.push(Reverse((Dist(d), n)));
                    results.push((Dist(d), n));
                    if results.len() > ef {
                        results.pop(); // 淘汰最差
                    }
                }
            }
        }

        let mut out: Vec<(u32, f32)> = results
            .into_iter()
            .map(|(Dist(d), n)| (n, d))
            .collect();
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
        out
    }

    /// 双向连边（idx 在第 level 层连接 selected；对端超 m 时按距自身最近截断）
    fn connect_bidirectional(&mut self, idx: u32, selected: &[u32], level: usize) {
        let qvec = self.nodes[idx as usize].vector.clone();
        // 1) 对端邻居表：拷贝后追加/截断（避免与 self 可变借用冲突）
        let mut updates: Vec<(u32, Vec<u32>)> = Vec::with_capacity(selected.len());
        for &n in selected {
            let nvec = self.nodes[n as usize].vector.clone();
            let mut nn = self.nodes[n as usize].neighbors[level].clone();
            nn.push(idx);
            if nn.len() > self.m {
                nn.sort_by(|a, b| {
                    distance(&self.nodes[*a as usize].vector, &nvec)
                        .partial_cmp(&distance(&self.nodes[*b as usize].vector, &nvec))
                        .unwrap_or(Ordering::Equal)
                });
                nn.truncate(self.m);
            }
            updates.push((n, nn));
        }
        for (n, nn) in updates {
            self.nodes[n as usize].neighbors[level] = nn;
        }
        // 2) 自身邻居：拷贝排序截断后写回
        let mut qn = self.nodes[idx as usize].neighbors[level].clone();
        qn.extend_from_slice(selected);
        if qn.len() > self.m {
            qn.sort_by(|a, b| {
                distance(&self.nodes[*a as usize].vector, &qvec)
                    .partial_cmp(&distance(&self.nodes[*b as usize].vector, &qvec))
                    .unwrap_or(Ordering::Equal)
            });
            qn.truncate(self.m);
        }
        self.nodes[idx as usize].neighbors[level] = qn;
    }
}

/// 距离 = 1 − 内积（向量已归一化 → 余弦距离）
fn distance(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
    }
    1.0 - dot
}

/// 可比较距离（f32 无 Ord/Eq，用 total_cmp 全序包装）
#[derive(Debug, Clone, Copy)]
struct Dist(f32);

impl PartialEq for Dist {
    fn eq(&self, other: &Self) -> bool {
        self.0.total_cmp(&other.0) == Ordering::Equal
    }
}
impl Eq for Dist {}

impl PartialOrd for Dist {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Dist {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kb::embed::dummy_vector;

    fn norm_vec(seed: u64, dim: usize) -> Vec<f32> {
        dummy_vector(&format!("hnsw-{seed}"), dim)
    }

    #[test]
    fn add_and_search_finds_identical_vector() {
        let mut h = HnswIndex::new(32);
        let v1 = norm_vec(1, 32);
        let v2 = norm_vec(2, 32);
        let v3 = norm_vec(3, 32);
        h.add(101, &v1);
        h.add(102, &v2);
        h.add(103, &v3);
        assert_eq!(h.len(), 3);

        let hits = h.search(&v1, 3, 50);
        assert_eq!(hits[0].0, 101, "同向量应最近");
        assert!(hits[0].1 < 1e-3, "同向量距离应≈0: {}", hits[0].1);
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn add_same_id_is_idempotent() {
        let mut h = HnswIndex::new(8);
        let v = norm_vec(9, 8);
        h.add(1, &v);
        h.add(1, &v);
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn search_recovers_true_nn_on_larger_set() {
        let mut h = HnswIndex::new(16);
        for i in 0..300u64 {
            h.add(i as i64, &norm_vec(i, 16));
        }
        // 全量暴力核对：随机抽 20 个探针，HNSW top1 必须等于暴力最近
        for probe in (0..300u64).step_by(15) {
            let q = norm_vec(probe, 16);
            let brute = (0..300u64)
                .map(|i| {
                    let v = norm_vec(i, 16);
                    let dot: f32 = v.iter().zip(q.iter()).map(|(a, b)| a * b).sum();
                    (1.0 - dot, i)
                })
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
                .unwrap();
            let hit = h.search(&q, 1, 150);
            assert_eq!(hit[0].0, brute.1 as i64, "探针 {probe} 应命中暴力最近");
        }
    }

    #[test]
    fn empty_and_single() {
        let h = HnswIndex::new(4);
        assert!(h.search(&[0.0; 4], 5, 10).is_empty());
        let mut h = HnswIndex::new(4);
        h.add(1, &[1.0, 0.0, 0.0, 0.0]);
        assert_eq!(h.search(&[1.0, 0.0, 0.0, 0.0], 5, 10)[0].0, 1);
    }
}
