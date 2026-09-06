//! HNSW 向量索引（自实现，见 `hnsw.rs`）与向量 BLOB 转换。
//!
//! 一致性模型：`kb_chunks.embedding` BLOB 是向量**权威存储**，HNSW 文件只是
//! **可重建缓存**（从 BLOB 全量重建永远兜底）；`kb_index_meta` 记录索引状态机
//! 与文件路径。检索前做一致性校验（chunk_count 对齐），文档增删后未重建会
//! 明确提示，杜绝"搜到已删内容"。
//!
//! 线程模型：`VectorIndex` 以 `Mutex<HnswIndex>` 串行化——写入由构建任务独占，
//! 查询是短临界区；索引进入共享缓存后不再写（删除/重建 = 整索引替换）。
//! 持久化用 bincode（索引全状态序列化，含向量快照，加载即用）。

use std::path::Path;
use std::sync::Mutex;

use super::hnsw::HnswIndex;

/// f32 向量 ↔ SQLite BLOB（小端字节序）
pub(crate) fn blob_from_vec(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub(crate) fn vec_from_blob(b: &[u8]) -> Result<Vec<f32>, String> {
    if b.len() % 4 != 0 {
        return Err(format!("向量 BLOB 长度非法: {} 字节", b.len()));
    }
    Ok(b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// HNSW 索引句柄：内部互斥的图索引（自实现，cosine 距离语义）
pub struct VectorIndex {
    inner: Mutex<HnswIndex>,
    pub dim: usize,
}

impl VectorIndex {
    /// 创建空索引
    pub fn new(dim: usize) -> Result<Self, String> {
        if dim == 0 {
            return Err("向量维度不能为 0".to_string());
        }
        Ok(Self {
            inner: Mutex::new(HnswIndex::new(dim)),
            dim,
        })
    }

    /// 追加一个向量（label = chunk rowid；幂等）
    pub fn add(&mut self, key: i64, vector: &[f32]) -> Result<(), String> {
        debug_assert_eq!(vector.len(), self.dim, "向量维度与索引不一致");
        self.inner
            .lock()
            .unwrap()
            .add(key, vector);
        Ok(())
    }

    /// 内存索引规模（供索引管理命令/状态展示；当前一致性校验走 SQL count）
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }

    /// ANN 检索：返回 (chunk_id, 余弦距离) 升序（距离越小越相似）
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(i64, f32)>, String> {
        let hits = self
            .inner
            .lock()
            .unwrap()
            .search(query, top_k, (top_k * 16).max(64));
        Ok(hits)
    }

    /// 持久化到文件（父目录自动创建；bincode 全状态快照）
    pub fn save(&self, path: &str) -> Result<(), String> {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建索引目录失败: {e}"))?;
            }
        }
        let data = bincode::serialize(&*self.inner.lock().unwrap())
            .map_err(|e| format!("序列化索引失败: {e}"))?;
        std::fs::write(path, &data).map_err(|e| format!("保存 HNSW 索引失败: {e}"))
    }

    /// 从文件加载（dim 校验由调用方按 index_meta 提供）
    pub fn load(path: &str, dim: usize) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("读取 HNSW 索引失败: {e}"))?;
        let inner: HnswIndex = bincode::deserialize(&data)
            .map_err(|e| format!("反序列化索引失败: {e}"))?;
        if inner.len() > 0 && dim != 0 {
            // 与元数据维度对齐（文件头无显式 dim 校验字段时以元数据为准）
            let _ = dim;
        }
        Ok(Self {
            inner: Mutex::new(inner),
            dim,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kb::embed::dummy_vector;

    fn fake_vec(seed: u64, dim: usize) -> Vec<f32> {
        dummy_vector(&format!("vec-{seed}"), dim)
    }

    #[test]
    fn blob_roundtrip() {
        let v = vec![1.5f32, -2.25, 0.0, 3.125];
        let b = blob_from_vec(&v);
        assert_eq!(b.len(), 16);
        let back = vec_from_blob(&b).unwrap();
        assert_eq!(v, back);
        assert!(vec_from_blob(&[1, 2, 3]).is_err());
    }

    #[test]
    fn build_search_finds_nearest() {
        let mut ix = VectorIndex::new(16).unwrap();
        let a = fake_vec(1, 16);
        let b = fake_vec(2, 16);
        let c = fake_vec(3, 16);
        ix.add(101, &a).unwrap();
        ix.add(102, &b).unwrap();
        ix.add(103, &c).unwrap();
        assert_eq!(ix.len(), 3);

        let hits = ix.search(&a, 3).unwrap();
        assert_eq!(hits[0].0, 101);
        assert!(hits[0].1.abs() < 1e-3, "相同向量距离应≈0: {}", hits[0].1);
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn save_load_roundtrip() {
        let mut ix = VectorIndex::new(8).unwrap();
        for i in 1i64..=30 {
            ix.add(i, &fake_vec(i as u64, 8)).unwrap();
        }
        let path = std::env::temp_dir().join(format!("hnsw-test-{}.idx", std::process::id()));
        let p = path.to_str().unwrap();
        ix.save(p).unwrap();

        let loaded = VectorIndex::load(p, 8).unwrap();
        assert_eq!(loaded.len(), 30);
        let q = fake_vec(7, 8);
        let hits = loaded.search(&q, 1).unwrap();
        assert_eq!(hits[0].0, 7, "加载后检索应一致");
        let _ = std::fs::remove_file(p);
    }
}
