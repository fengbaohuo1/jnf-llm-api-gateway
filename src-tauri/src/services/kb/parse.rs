//! 文档解析与分块（纯函数，无 IO，便于单测）。
//!
//! 理念（借鉴 markitdown 的"归一化中间表示"）：各格式解析器统一输出
//! **章节流**（heading 溯源路径 + 正文），分块器再把章节切成检索块；
//! 下游入库/检索不感知原始格式。当前支持 md（结构化）与 txt；pdf/docx 等
//! 后续接入时只需新增一个"格式 → 章节流"解析器。
//!
//! 分块策略（递归字符分割的简化两级版）：
//! - 章节不超上限 → 整体成块（保留结构边界与标题溯源）；
//! - 超限 → 先按段落（空行分隔）贪心组合到接近上限，切块只发生在段落边界
//!   （段落是语义完整的最小单位，不腰斩）；
//! - 单个段落仍超限（无结构可依）→ 重叠硬窗口切分（overlap 保上下文衔接）。

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// 解析后的章节：heading 为溯源路径（多级标题 "A / B"），无标题为空串
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub heading: String,
    pub text: String,
}

/// 待入库的检索块
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedChunk {
    pub heading: String,
    pub content: String,
}

/// 按格式分发解析（当前支持 md/markdown 结构化解析；其余按纯文本）
pub fn parse_by_format(format: &str, text: &str) -> Vec<Section> {
    match format {
        "md" | "markdown" => parse_markdown(text),
        _ => parse_plain(text),
    }
}

/// 解析纯文本：整文一个无标题章节
pub fn parse_plain(text: &str) -> Vec<Section> {
    let text = text.trim();
    if text.is_empty() {
        Vec::new()
    } else {
        vec![Section {
            heading: String::new(),
            text: text.to_string(),
        }]
    }
}

/// 解析 markdown → 章节流。
/// 用 pulldown-cmark 事件流而非行扫描：代码围栏内的 `#` 不会被误判为标题，
/// 代码块/表格等块结构保持为正文内容。标题层级维护为"标题栈"，正文挂到
/// 栈快照上，得到多级溯源路径。
pub fn parse_markdown(text: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new(); // (标题级别, 标题文本)
    let mut in_heading = false;
    let mut buf = String::new();
    let mut para_break = false; // 段落已结束，等下一段文本前补空行

    for ev in Parser::new_ext(text, Options::all()) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                // 新标题：把此前累积的正文落为一个章节
                flush(&mut sections, &stack, &mut buf);
                // 同级或更深的标题出栈（保持祖先链）；新标题入栈待文本填充
                let lvl = level_num(level);
                while stack.last().is_some_and(|(l, _)| *l >= lvl) {
                    stack.pop();
                }
                stack.push((lvl, String::new()));
                in_heading = true;
                para_break = false;
            }
            Event::Text(t) if in_heading => {
                // 标题文本（可能跨多个 Text 事件，如行内强调），空格拼接
                if let Some((_, title)) = stack.last_mut() {
                    if !title.is_empty() {
                        title.push(' ');
                    }
                    title.push_str(&t);
                }
            }
            Event::Text(t) => {
                // 正文：段落边界补空行（保留结构，供段落级分块）
                if para_break && !buf.is_empty() {
                    if !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                    buf.push('\n');
                }
                para_break = false;
                buf.push_str(&t);
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                if !buf.is_empty() {
                    if !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                    buf.push('\n');
                }
                para_break = false;
            }
            Event::End(TagEnd::CodeBlock) => {
                if !buf.ends_with('\n') {
                    buf.push('\n');
                }
                para_break = false;
            }
            Event::End(TagEnd::Paragraph) => {
                para_break = true;
            }
            _ => {}
        }
    }
    flush(&mut sections, &stack, &mut buf);
    sections
}

/// 章节流 → 分块（策略见模块注释）
pub fn split_sections(sections: Vec<Section>, size: usize, overlap: usize) -> Vec<ParsedChunk> {
    let size = size.clamp(100, 100_000);
    let overlap = overlap.min(size / 2);
    let mut out = Vec::new();
    for sec in sections {
        let text = sec.text.trim();
        if text.is_empty() {
            continue;
        }
        if text.chars().count() <= size {
            out.push(ParsedChunk {
                heading: sec.heading.clone(),
                content: text.to_string(),
            });
            continue;
        }
        // 超限：段落贪心组合
        let paragraphs: Vec<&str> = text
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let mut cur = String::new();
        let mut cur_chars = 0usize;
        for p in paragraphs {
            let p_chars = p.chars().count();
            if p_chars > size {
                // 单段超限：先落当前累积，再重叠硬窗切该段
                if !cur.trim().is_empty() {
                    out.push(ParsedChunk {
                        heading: sec.heading.clone(),
                        content: cur.trim().to_string(),
                    });
                }
                cur.clear();
                cur_chars = 0;
                for piece in hard_split(p, size, overlap) {
                    out.push(ParsedChunk {
                        heading: sec.heading.clone(),
                        content: piece,
                    });
                }
                continue;
            }
            if !cur.is_empty() && cur_chars + 1 + p_chars > size {
                // 段落溢出：当前块落库，从新段落重新累积——段落是语义完整的
                // 最小单位，切块只发生在段落边界（overlap 仅用于无结构硬窗，
                // 见 split_sections 注释），避免把段落腰斩成半个。
                out.push(ParsedChunk {
                    heading: sec.heading.clone(),
                    content: cur.trim().to_string(),
                });
                cur.clear();
                cur_chars = 0;
            }
            if !cur.is_empty() {
                cur.push('\n');
                cur_chars += 1;
            }
            cur.push_str(p);
            cur_chars += p_chars;
        }
        if !cur.trim().is_empty() {
            out.push(ParsedChunk {
                heading: sec.heading,
                content: cur.trim().to_string(),
            });
        }
    }
    out
}

/// 把缓冲的正文落成章节（空白丢弃），并清空缓冲
fn flush(sections: &mut Vec<Section>, stack: &[(usize, String)], buf: &mut String) {
    let text = buf.trim().to_string();
    if !text.is_empty() {
        let heading = stack
            .iter()
            .map(|(_, t)| t.as_str())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        sections.push(Section { heading, text });
    }
    buf.clear();
}

fn level_num(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// 重叠硬窗口切分（字符级）
fn hard_split(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let end = (i + size).min(chars.len());
        pieces.push(chars[i..end].iter().collect::<String>().trim().to_string());
        if end == chars.len() {
            break;
        }
        if overlap >= size {
            break;
        }
        i = end.saturating_sub(overlap);
    }
    pieces.into_iter().filter(|p| !p.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n_chars(s: &str) -> usize {
        s.chars().count()
    }

    /// 取字符串最后 n 个字符（测试断言用）
    fn tail_chars(s: &str, n: usize) -> String {
        let chars: Vec<char> = s.chars().collect();
        let start = chars.len().saturating_sub(n);
        chars[start..].iter().collect()
    }

    #[test]
    fn md_sections_follow_heading_tree_with_nested_path() {
        let md = "# 第一章 概述\n\n机器学习是一门学科。\n\n## 1.1 历史\n\n深度学习兴起于 2012 年。\n\n# 第二章 总结\n\n全书到此结束。";
        let sections = parse_markdown(md);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].heading, "第一章 概述");
        assert!(sections[0].text.contains("机器学习是一门学科"));
        // 嵌套标题保留祖先路径（溯源用）
        assert_eq!(sections[1].heading, "第一章 概述 / 1.1 历史");
        assert!(sections[1].text.contains("2012"));
        assert_eq!(sections[2].heading, "第二章 总结");
        assert!(sections[2].text.contains("全书到此结束"));
    }

    #[test]
    fn md_fenced_code_hash_is_not_a_heading() {
        let md = "## 示例\n\n```text\n# 这不是标题\n## 也不是\n```\n\n代码之后正文。";
        let sections = parse_markdown(md);
        assert_eq!(sections.len(), 1, "代码围栏内的 # 行不应产生新章节");
        assert_eq!(sections[0].heading, "示例");
        assert!(
            sections[0].text.contains("# 这不是标题"),
            "围栏内容应保留在正文"
        );
        assert!(
            sections[0].text.contains("代码之后正文"),
            "围栏后正文应并入同章节"
        );
    }

    #[test]
    fn md_heading_swap_replaces_same_level() {
        let md = "# A\n\na 正文\n\n# B\n\nb 正文";
        let sections = parse_markdown(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "A");
        assert_eq!(sections[1].heading, "B");
    }

    #[test]
    fn plain_text_is_one_section_without_heading() {
        let sections = parse_plain("  只有一段话\n没有标题  ");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading, "");
        assert_eq!(sections[0].text, "只有一段话\n没有标题");
        assert!(parse_plain("   ").is_empty());
    }

    #[test]
    fn small_section_stays_whole() {
        let md = "# 标题\n\n短正文。";
        let chunks = split_sections(parse_markdown(md), 800, 150);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading, "标题");
        assert_eq!(chunks[0].content, "短正文。");
    }

    #[test]
    fn oversized_paragraph_gets_overlapped_windows() {
        // 单段超长（无空行可切）→ 重叠硬窗
        let body = "汉".repeat(1000);
        let md = format!("# T\n\n{body}");
        let chunks = split_sections(parse_markdown(&md), 300, 60);
        assert!(chunks.len() >= 4, "1000 字符 / 300 窗口应至少 4 块，实际 {}", chunks.len());
        // 相邻块共享重叠尾（第 2 块以第 1 块尾部字符开头）
        let c1 = chunks[0].content.clone();
        let c2 = &chunks[1].content;
        let tail = tail_chars(&c1, 60);
        assert!(
            c2.starts_with(&tail),
            "下一块应以重叠尾开头（块间无缝隙）"
        );
        // 各块不超上限
        for c in &chunks {
            assert!(n_chars(&c.content) <= 300 + 1, "块长 {} 超过窗口", n_chars(&c.content));
        }
    }

    #[test]
    fn oversized_section_splits_by_paragraphs() {
        // 20 段 × 50 字符，上限 400：贪心按段落组合成约 3-4 块，段落不切碎
        let mut body = String::new();
        for _ in 0..20 {
            body.push_str(&"段".repeat(50));
            body.push_str("\n\n");
        }
        let md = format!("# T\n\n{body}");
        let sections = parse_markdown(&md);
        assert_eq!(sections.len(), 1);
        // 段落边界（空行）必须保留，段落级分块才成立
        let paras: Vec<&str> = sections[0].text.split("\n\n").collect();
        assert_eq!(paras.len(), 20, "段落空行丢失：{:?}", &sections[0].text[..200.min(sections[0].text.len())]);

        let chunks = split_sections(sections, 400, 80);
        assert!(chunks.len() >= 3, "20 段 × 50 字符 / 400 上限应拆出至少 3 块，实际 {}", chunks.len());
        for c in &chunks {
            assert!(n_chars(&c.content) <= 420, "块异常超长: {}", n_chars(&c.content));
        }
        // 段落是 50 的整数倍 → 组合块不把段落切碎（无 <50 字符的碎块）
        for c in &chunks {
            let lines: Vec<&str> = c.content.lines().filter(|l| !l.is_empty()).collect();
            for l in lines {
                assert_eq!(l.chars().count(), 50, "段落被切碎: {l:?}");
            }
        }
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(split_sections(parse_markdown(""), 800, 150).is_empty());
        assert!(split_sections(parse_markdown("# 只有标题"), 800, 150).is_empty());
    }
}
