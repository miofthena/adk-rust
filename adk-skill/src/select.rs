use crate::model::{SelectionPolicy, SkillIndex, SkillMatch, SkillSummary};
use std::collections::HashSet;

/// Selects the most relevant skills from the index for a given query string.
///
/// Skills are scored by token overlap across name, description, tags, and body,
/// filtered by the include/exclude tag lists in `policy`, and returned in
/// descending score order up to `policy.top_k` results. Only matches that meet
/// `policy.min_score` are included.
pub fn select_skills(index: &SkillIndex, query: &str, policy: &SelectionPolicy) -> Vec<SkillMatch> {
    if policy.top_k == 0 {
        return Vec::new();
    }

    let include_tags =
        policy.include_tags.iter().map(|t| t.to_ascii_lowercase()).collect::<HashSet<_>>();
    let exclude_tags =
        policy.exclude_tags.iter().map(|t| t.to_ascii_lowercase()).collect::<HashSet<_>>();

    let query_tokens = tokenize(query);
    if query_tokens.is_empty() && include_tags.is_empty() {
        return Vec::new();
    }

    let mut scored = index
        .skills()
        .iter()
        .filter(|skill| tag_allowed(skill, &include_tags, &exclude_tags))
        .map(|skill| {
            let score = score_skill(&query_tokens, skill);
            SkillMatch { score, skill: SkillSummary::from(skill) }
        })
        .filter(|m| m.score >= policy.min_score)
        .collect::<Vec<_>>();

    scored.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.skill.name.cmp(&b.skill.name))
            .then_with(|| a.skill.path.cmp(&b.skill.path))
    });

    scored.into_iter().take(policy.top_k).collect()
}

fn tag_allowed(
    skill: &crate::model::SkillDocument,
    include: &HashSet<String>,
    exclude: &HashSet<String>,
) -> bool {
    let skill_tags = skill.tags.iter().map(|t| t.to_ascii_lowercase()).collect::<HashSet<_>>();

    if !exclude.is_empty() && !skill_tags.is_disjoint(exclude) {
        return false;
    }

    include.is_empty() || !skill_tags.is_disjoint(include)
}

fn score_skill(query_tokens: &[String], skill: &crate::model::SkillDocument) -> f32 {
    let name_tokens = to_set(&skill.name);
    let description_tokens = to_set(&skill.description);
    let body_tokens = to_set(&skill.body);
    let tags_tokens = skill.tags.iter().flat_map(|t| tokenize(t)).collect::<HashSet<_>>();

    let mut score = 0.0;
    for token in query_tokens {
        if name_tokens.contains(token) {
            score += 4.0;
        }
        if description_tokens.contains(token) {
            score += 2.5;
        }
        if tags_tokens.contains(token) {
            score += 2.0;
        }
        if body_tokens.contains(token) {
            score += 1.0;
        }
    }

    // Small normalization to avoid bias toward huge docs while keeping scoring simple.
    let norm = (body_tokens.len().max(1) as f32).sqrt();
    score / norm.max(1.0)
}

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            // ASCII alphanumeric: fold to lowercase for case-insensitive matching
            current.push(c.to_ascii_lowercase());
        } else if is_unicode_alphanumeric(c) {
            // Non-ASCII alphanumeric (CJK, Cyrillic, Arabic, accented Latin, etc.):
            // flush any pending ASCII token, then emit the character as its own token.
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            tokens.push(c.to_string());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Returns `true` if `c` is a Unicode alphabetic or numeric character that is
/// not ASCII alphanumeric. This covers CJK ideographs, kana, hangul, Cyrillic,
/// Arabic, accented Latin, and any other script classified as Alphabetic or
/// Numeric by Unicode.
fn is_unicode_alphanumeric(c: char) -> bool {
    if (c as u32) < 0x80 {
        return false; // ASCII handled by is_ascii_alphanumeric
    }
    c.is_alphanumeric()
}

fn to_set(input: &str) -> HashSet<String> {
    tokenize(input).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::load_skill_index;
    use std::fs;

    #[test]
    fn selects_most_relevant_skill() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".skills")).unwrap();

        fs::write(
            root.join(".skills/code_search.md"),
            "---\nname: code_search\ndescription: Search Rust code with rg\ntags: [code, search]\n---\nUse rg --files then rg.",
        )
        .unwrap();
        fs::write(
            root.join(".skills/release_notes.md"),
            "---\nname: release_notes\ndescription: Prepare release notes\ntags: [changelog]\n---\nSummarize commits.",
        )
        .unwrap();

        let index = load_skill_index(root).unwrap();
        let policy = SelectionPolicy { top_k: 1, min_score: 0.1, ..SelectionPolicy::default() };
        let matches = select_skills(&index, "search rust codebase", &policy);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].skill.name, "code_search");
    }

    #[test]
    fn returns_empty_when_unrelated() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".skills")).unwrap();

        fs::write(
            root.join(".skills/release.md"),
            "---\nname: release\ndescription: Release process\n---\nBump versions and publish.",
        )
        .unwrap();

        let index = load_skill_index(root).unwrap();
        let policy = SelectionPolicy { top_k: 1, min_score: 2.0, ..SelectionPolicy::default() };
        let matches = select_skills(&index, "quantum entanglement", &policy);
        assert!(matches.is_empty());
    }

    #[test]
    fn include_and_exclude_tags_filter_results() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".skills")).unwrap();

        fs::write(
            root.join(".skills/code.md"),
            "---\nname: code\ndescription: code search\ntags: [code, search]\n---\nUse rg.\n",
        )
        .unwrap();
        fs::write(
            root.join(".skills/release.md"),
            "---\nname: release\ndescription: release notes\ntags: [docs]\n---\nSummarize commits.\n",
        )
        .unwrap();

        let index = load_skill_index(root).unwrap();
        let policy = SelectionPolicy {
            top_k: 5,
            min_score: 0.1,
            include_tags: vec!["code".to_string()],
            exclude_tags: vec!["docs".to_string()],
        };

        let matches = select_skills(&index, "search", &policy);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].skill.name, "code");
    }

    // ── Unicode / CJK support ─────────────────────────────────────────

    fn chinese_skill_index() -> SkillIndex {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".skills")).unwrap();

        fs::write(
            root.join(".skills/电脑操作.md"),
            "---\nname: 电脑操作\ndescription: 控制桌面电脑截屏点击打字滚动管理窗口\ntags: [桌面, 自动化]\n---\n你是桌面控制智能体。先截图再看屏幕。\n",
        )
        .unwrap();

        fs::write(
            root.join(".skills/数据库操作.md"),
            "---\nname: 数据库操作\ndescription: 查询数据库检查表结构分析性能管理迁移\ntags: [数据库, sql]\n---\n你是数据库运维专家。只读查询默认。\n",
        )
        .unwrap();

        fs::write(
            root.join(".skills/computer-use.md"),
            "---\nname: computer-use\ndescription: Control desktop computer screenshot click type scroll\ntags: [desktop, automation]\n---\nYou control the desktop. Screenshot first.\n",
        )
        .unwrap();

        load_skill_index(root).unwrap()
    }

    #[test]
    fn chinese_query_selects_chinese_skill() {
        let index = chinese_skill_index();
        let policy = SelectionPolicy { top_k: 1, min_score: 0.1, ..SelectionPolicy::default() };
        let matches = select_skills(&index, "截图给我看看屏幕", &policy);
        assert!(!matches.is_empty(), "Chinese query must return at least one match");
        assert_eq!(matches[0].skill.name, "电脑操作");
    }

    #[test]
    fn chinese_database_query_selects_database_skill() {
        let index = chinese_skill_index();
        let policy = SelectionPolicy { top_k: 1, min_score: 0.1, ..SelectionPolicy::default() };
        let matches = select_skills(&index, "查询数据库性能", &policy);
        assert!(!matches.is_empty(), "Chinese DB query must return at least one match");
        assert_eq!(matches[0].skill.name, "数据库操作");
    }

    #[test]
    fn english_query_still_matches_english_skill() {
        let index = chinese_skill_index();
        let policy = SelectionPolicy { top_k: 1, min_score: 0.1, ..SelectionPolicy::default() };
        let matches = select_skills(&index, "screenshot desktop control", &policy);
        assert!(!matches.is_empty(), "English query must return at least one match");
        assert_eq!(matches[0].skill.name, "computer-use");
    }

    #[test]
    fn tokenize_handles_cjk_and_ascii_mixed() {
        let tokens = tokenize("打开 Safari 浏览器");
        assert!(tokens.contains(&"打".to_string()));
        assert!(tokens.contains(&"开".to_string()));
        assert!(tokens.contains(&"safari".to_string()));
        assert!(tokens.contains(&"浏".to_string()));
        assert!(tokens.contains(&"览".to_string()));
        assert!(tokens.contains(&"器".to_string()));
    }

    #[test]
    fn tokenize_handles_pure_ascii_as_before() {
        let tokens = tokenize("search the codebase");
        assert_eq!(tokens, vec!["search", "the", "codebase"]);
    }

    #[test]
    fn tokenize_handles_empty_input() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   ").is_empty());
        assert!(tokenize("!@#$%").is_empty());
    }

    #[test]
    fn tokenize_handles_cyrillic() {
        let tokens = tokenize("поиск база данных");
        assert!(!tokens.is_empty());
        assert!(tokens.contains(&"п".to_string()));
        assert!(tokens.contains(&"о".to_string()));
    }

    #[test]
    fn tokenize_handles_accented_latin() {
        let tokens = tokenize("déjà vu");
        assert!(tokens.contains(&"d".to_string()));
        assert!(tokens.contains(&"é".to_string()));
        assert!(tokens.contains(&"j".to_string()));
        assert!(tokens.contains(&"à".to_string()));
        assert!(tokens.contains(&"vu".to_string()));
    }

    #[test]
    fn japanese_query_selects_japanese_skill() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".skills")).unwrap();

        fs::write(
            root.join(".skills/タスク管理.md"),
            "---\nname: タスク管理\ndescription: タスクの作成更新削除優先度管理進捗追跡\ntags: [タスク, 管理]\n---\nあなたはタスク管理アシスタントです。\n",
        )
        .unwrap();

        fs::write(
            root.join(".skills/other.md"),
            "---\nname: other\ndescription: Something else entirely\ntags: [misc]\n---\nGeneric skill.\n",
        )
        .unwrap();

        let index = load_skill_index(root).unwrap();
        let policy = SelectionPolicy { top_k: 1, min_score: 0.1, ..SelectionPolicy::default() };
        let matches = select_skills(&index, "タスクの優先度を変更して", &policy);
        assert!(!matches.is_empty(), "Japanese query must match");
        assert_eq!(matches[0].skill.name, "タスク管理");
    }

    #[test]
    fn korean_query_selects_korean_skill() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".skills")).unwrap();

        fs::write(
            root.join(".skills/일정-관리.md"),
            "---\nname: 일정-관리\ndescription: 일정 생성 수정 삭제 충돌 확인 알림 관리\ntags: [일정, 캘린더]\n---\n당신은 일정 관리 도우미입니다.\n",
        )
        .unwrap();

        fs::write(
            root.join(".skills/other.md"),
            "---\nname: other\ndescription: Unrelated skill\ntags: [misc]\n---\nNothing here.\n",
        )
        .unwrap();

        let index = load_skill_index(root).unwrap();
        let policy = SelectionPolicy { top_k: 1, min_score: 0.1, ..SelectionPolicy::default() };
        let matches = select_skills(&index, "내일 일정 확인해줘", &policy);
        assert!(!matches.is_empty(), "Korean query must match");
        assert_eq!(matches[0].skill.name, "일정-관리");
    }

    #[test]
    fn tokenize_handles_japanese_kana_and_kanji() {
        let tokens = tokenize("タスクを作成する");
        // Katakana and Kanji are non-ASCII alphanumeric
        assert!(tokens.contains(&"タ".to_string()));
        assert!(tokens.contains(&"ス".to_string()));
        assert!(tokens.contains(&"ク".to_string()));
        assert!(tokens.contains(&"作".to_string()));
        assert!(tokens.contains(&"成".to_string()));
    }

    #[test]
    fn tokenize_handles_korean_hangul() {
        let tokens = tokenize("일정 관리");
        assert!(tokens.contains(&"일".to_string()));
        assert!(tokens.contains(&"정".to_string()));
        assert!(tokens.contains(&"관".to_string()));
        assert!(tokens.contains(&"리".to_string()));
    }
}
