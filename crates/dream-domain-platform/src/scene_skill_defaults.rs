//! Default skill package of the built-in scenes.
//!
//! A built-in scene used to be seeded with a `'*'` (all skills) grant. That is the
//! wrong default twice over: it names no skill a member's scene conversation can
//! actually select (the client only turns *explicit* skill ids into the
//! conversation's skills), and "everything" is the opposite of a scene's purpose.
//!
//! The default is now the skills that best fit the scene, at most
//! [`MAX_DEFAULT_SCENE_SKILLS`]. This is a default only: an administrator can
//! grant any number of skills (or all of them) afterwards.
//!
//! Selection is deterministic: first the scene's curated official skills, in the
//! listed order; then, while room is left, the market skills that clearly match
//! the scene (preferred category plus keyword hits), best score first, ties by
//! name. A market skill below [`FILL_MIN_SCORE`] is never recommended: a short
//! package is better than a padded one.

/// Upper bound of the *default* package; not a limit on what an admin may grant.
pub const MAX_DEFAULT_SCENE_SKILLS: usize = 10;

/// A market skill must score at least this to be recommended (e.g. a preferred
/// category plus one name hit, or two name hits).
const FILL_MIN_SCORE: i32 = 9;

/// One skill the recommender may pick: published, enabled and visible to members.
#[derive(Debug, Clone)]
pub struct SkillCandidate {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Category name; empty when uncategorised.
    pub category: String,
    /// `catalog` for the official built-ins, `market` / `custom` otherwise.
    pub origin: String,
}

struct SceneProfile {
    scene: &'static str,
    /// Official skills, in priority order, matched by exact name.
    curated: &'static [&'static str],
    /// Category names that raise a market skill's score.
    categories: &'static [&'static str],
    /// Lower-case keywords matched against name, category and description.
    keywords: &'static [&'static str],
}

const PROFILES: [SceneProfile; 5] = [
    SceneProfile {
        scene: "办公",
        curated: &[
            "onework-meeting-notes-guide",
            "onework-weekly-report-guide",
            "onework-admin-procurement-guide",
            "onework-travel-expense-guide",
            "onework-hr-assistant-guide",
            "onework-onboarding-guide-guide",
            "onework-contract-review-guide",
            "onework-data-report-guide",
        ],
        categories: &["办公", "效率工具", "生活方式"],
        keywords: &[
            "office",
            "word",
            "excel",
            "pdf",
            "ppt",
            "docx",
            "xlsx",
            "powerpoint",
            "calendar",
            "meeting",
            "notion",
            "todo",
            "文档",
            "表格",
            "演示",
            "日程",
            "会议",
            "笔记",
            "待办",
            "办公",
            "飞书",
            "feishu",
            "lark",
        ],
    },
    SceneProfile {
        scene: "IT运维",
        curated: &[
            "onework-it-helpdesk-guide",
            "onework-k8s-ops-guide",
            "onework-prometheus-guide",
            "onework-zabbix-guide",
            "onework-elk-guide",
            "onework-nginx-guide",
            "onework-ansible-guide",
            "onework-terraform-guide",
            "onework-argocd-guide",
            "onework-jenkins-guide",
        ],
        categories: &[
            "DevOps",
            "基础设施",
            "监控与可观测性",
            "Kubernetes",
            "日志分析",
            "安全运维",
            "CI/CD",
            "配置管理",
        ],
        keywords: &[
            "ops",
            "运维",
            "kubernetes",
            "k8s",
            "docker",
            "zabbix",
            "prometheus",
            "grafana",
            "nginx",
            "linux",
            "ansible",
            "terraform",
            "helm",
            "argocd",
            "monitor",
            "监控",
            "巡检",
            "故障",
            "告警",
        ],
    },
    SceneProfile {
        scene: "网络安全",
        curated: &[
            "onework-vault-guide",
            "onework-harbor-guide",
            "onework-elk-guide",
            "onework-gitlab-guide",
        ],
        categories: &["网络与安全", "测试与安全", "安全运维"],
        keywords: &[
            "security",
            "secur",
            "安全",
            "vulnerab",
            "漏洞",
            "threat",
            "威胁",
            "waf",
            "siem",
            "审计",
            "audit",
            "pentest",
            "渗透",
            "incident",
            "应急",
            "合规",
            "compliance",
            "firewall",
            "防火墙",
            "malware",
            "sast",
            "cve",
        ],
    },
    SceneProfile {
        scene: "新媒体运营",
        curated: &["onework-seo-planner-guide", "onework-competitor-research-guide"],
        categories: &["内容与媒体"],
        keywords: &[
            "content",
            "内容",
            "social",
            "社媒",
            "xiaohongshu",
            "小红书",
            "douyin",
            "抖音",
            "wechat",
            "公众号",
            "weibo",
            "微博",
            "video",
            "视频",
            "seo",
            "copywrit",
            "文案",
            "twitter",
            "youtube",
            "tiktok",
            "bilibili",
            "blog",
            "选题",
            "运营",
        ],
    },
    SceneProfile {
        scene: "市场营销",
        curated: &[
            "onework-competitor-research-guide",
            "onework-seo-planner-guide",
            "onework-data-report-guide",
        ],
        categories: &["业务应用", "内容与媒体"],
        keywords: &[
            "marketing",
            "营销",
            "campaign",
            "广告",
            "ads",
            "analytics",
            "分析",
            "brand",
            "品牌",
            "growth",
            "增长",
            "crm",
            "销售",
            "sales",
            "competitor",
            "竞品",
            "persona",
            "用户画像",
            "市场",
            "lead",
        ],
    },
];

fn score(profile: &SceneProfile, c: &SkillCandidate) -> i32 {
    let name = c.name.to_lowercase();
    let description = c.description.to_lowercase();
    let category = c.category.to_lowercase();
    let mut s = if profile.categories.contains(&c.category.as_str()) {
        3
    } else {
        0
    };
    for k in profile.keywords {
        if name.contains(k) {
            s += 4;
        }
        if category.contains(k) {
            s += 1;
        }
        if description.contains(k) {
            s += 1;
        }
    }
    s
}

/// Ids of the skills recommended as the default package of the built-in scene
/// `scene_name`; empty for a scene without a profile or when nothing fits.
pub fn recommend(scene_name: &str, candidates: &[SkillCandidate]) -> Vec<String> {
    let Some(profile) = PROFILES.iter().find(|p| p.scene == scene_name) else {
        return Vec::new();
    };
    let mut picked: Vec<&SkillCandidate> = Vec::new();
    for name in profile.curated {
        if picked.len() >= MAX_DEFAULT_SCENE_SKILLS {
            break;
        }
        if let Some(c) = candidates.iter().find(|c| c.name == *name) {
            picked.push(c);
        }
    }
    if picked.len() < MAX_DEFAULT_SCENE_SKILLS {
        let mut scored: Vec<(i32, &SkillCandidate)> = candidates
            .iter()
            .filter(|c| c.origin != "catalog" && !picked.iter().any(|p| p.id == c.id))
            .map(|c| (score(profile, c), c))
            .filter(|(s, _)| *s >= FILL_MIN_SCORE)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        for (_, c) in scored {
            if picked.len() >= MAX_DEFAULT_SCENE_SKILLS {
                break;
            }
            picked.push(c);
        }
    }
    picked.into_iter().map(|c| c.id.clone()).collect()
}

/// Keywords derive from the scene identity and its standing prompt, not all tools.
pub(crate) fn mcp_terms(name: &str, prompt: &str) -> Vec<String> {
    let text = format!("{name} {prompt}").to_lowercase();
    let mut terms = std::collections::BTreeSet::new();
    for profile in &PROFILES {
        for keyword in profile.keywords {
            if name.contains(profile.scene) || text.contains(keyword) {
                terms.insert((*keyword).to_owned());
            }
        }
    }
    for word in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        if word.len() >= 3
            && ![
                "the", "and", "you", "for", "with", "this", "that", "are", "your", "use", "will", "from",
            ]
            .contains(&word)
        {
            terms.insert(word.into());
        }
    }
    for word in ["数据库", "搜索", "浏览器", "文件", "财务", "客户", "邮件"] {
        if text.contains(word) {
            terms.insert(word.into());
        }
    }
    terms.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(id: &str, name: &str, category: &str, origin: &str) -> SkillCandidate {
        SkillCandidate {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            category: category.into(),
            origin: origin.into(),
        }
    }

    #[test]
    fn curated_official_skills_come_first_in_listed_order() {
        let candidates = vec![
            skill("b", "onework-zabbix-guide", "", "catalog"),
            skill("a", "onework-k8s-ops-guide", "", "catalog"),
            skill("x", "random-skill", "", "market"),
        ];
        // The profile order (k8s before zabbix) wins over the candidate order.
        assert_eq!(recommend("IT运维", &candidates), vec!["a", "b"]);
    }

    #[test]
    fn never_more_than_the_default_cap_even_with_hundreds_of_matches() {
        let candidates: Vec<SkillCandidate> = (0..300)
            .map(|i| skill(&format!("s{i}"), &format!("k8s-ops-{i:03}"), "Kubernetes", "market"))
            .collect();
        let picked = recommend("IT运维", &candidates);
        assert_eq!(picked.len(), MAX_DEFAULT_SCENE_SKILLS);
        // Ties on score break by name, so the result is stable across runs and databases.
        assert_eq!(picked[0], "s0");
        assert_eq!(picked[9], "s9");
    }

    #[test]
    fn weak_matches_are_not_padded_in() {
        let candidates = vec![
            // preferred category alone (3) or one description hit never reaches the bar
            skill("w1", "weekly-notes", "效率工具", "market"),
            skill("w2", "unrelated", "", "market"),
        ];
        assert!(recommend("办公", &candidates).is_empty());
    }

    #[test]
    fn a_clear_market_match_fills_the_remaining_room_but_catalog_skills_outside_the_profile_do_not() {
        let candidates = vec![
            skill("c1", "onework-vault-guide", "", "catalog"),
            skill("m1", "openclaw-security-audit", "测试与安全", "market"),
            // an official skill of another scene must not leak in via keywords
            skill("c2", "onework-k8s-ops-guide", "", "catalog"),
        ];
        assert_eq!(recommend("网络安全", &candidates), vec!["c1", "m1"]);
    }

    #[test]
    fn unknown_scene_and_empty_catalogue_recommend_nothing() {
        let candidates = vec![skill("a", "onework-k8s-ops-guide", "", "catalog")];
        assert!(recommend("自定义场景", &candidates).is_empty());
        assert!(recommend("IT运维", &[]).is_empty());
    }

    #[test]
    fn every_profile_respects_the_cap_by_construction() {
        for p in &PROFILES {
            assert!(p.curated.len() <= MAX_DEFAULT_SCENE_SKILLS, "{}", p.scene);
        }
    }
}
