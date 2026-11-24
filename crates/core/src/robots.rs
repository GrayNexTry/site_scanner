//! robots.txt: решения «можно/нельзя» — порт парсера Google (robotstxt, RFC 9309),
//! плюс собственный разбор директив (группы, Sitemap, Host, Clean-param).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RobotsGroup {
    /// Агенты группы в нижнем регистре.
    pub agents: Vec<String>,
    /// В группе есть `Disallow: /`.
    pub disallow_all: bool,
    /// В группе есть `Allow: /` (перекрывает запрет на корень).
    pub allow_root: bool,
    pub rules: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RobotsDirectives {
    pub sitemaps: Vec<String>,
    pub host: bool,
    pub crawl_delay: bool,
    pub clean_param: bool,
    pub groups: Vec<RobotsGroup>,
    /// Строки с неизвестными директивами или без двоеточия.
    pub unknown_lines: usize,
}

impl RobotsDirectives {
    /// Группа, где агент назван явно (без учёта `*`).
    pub fn explicit_group(&self, agent: &str) -> Option<&RobotsGroup> {
        let a = agent.to_ascii_lowercase();
        self.groups.iter().find(|g| g.agents.contains(&a))
    }
}

pub fn parse(body: &str) -> RobotsDirectives {
    let mut d = RobotsDirectives::default();
    let mut in_rules = false;
    for raw in body.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            d.unknown_lines += 1;
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        match key.as_str() {
            "user-agent" => {
                if in_rules || d.groups.is_empty() {
                    d.groups.push(RobotsGroup::default());
                    in_rules = false;
                }
                if let Some(g) = d.groups.last_mut() {
                    g.agents.push(val.to_ascii_lowercase());
                }
            }
            "allow" | "disallow" => {
                in_rules = true;
                if let Some(g) = d.groups.last_mut() {
                    g.rules += 1;
                    if val == "/" {
                        if key == "disallow" {
                            g.disallow_all = true;
                        } else {
                            g.allow_root = true;
                        }
                    }
                }
            }
            "sitemap" => {
                // В значении URL с двоеточием: берём всё после первого «:».
                let url = line[line.find(':').map(|i| i + 1).unwrap_or(0)..].trim();
                if !url.is_empty() {
                    d.sitemaps.push(url.to_string());
                }
            }
            "host" => d.host = true,
            "crawl-delay" => d.crawl_delay = true,
            "clean-param" => d.clean_param = true,
            "request-rate" | "visit-time" | "noindex" => {}
            _ => d.unknown_lines += 1,
        }
    }
    d
}

/// Разрешён ли `url` агенту `agent` (семантика Google: самое длинное совпадение, `*`, `$`).
/// Адрес страницы без параметров и якоря. Ссылка из карточки часто с метками
/// (`/?utm_source=fb`), а правило `Disallow: /*?` закрывает именно адреса с параметрами —
/// о доступности самой главной это ничего не говорит.
pub fn page_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => url.split(['?', '#']).next().unwrap_or(url).to_string(),
    }
}

/// Группа robots.txt: имена ботов и правила (Allow — true, шаблон).
type Group = (Vec<String>, Vec<(bool, String)>);

/// Разрешён ли адрес для бота — по правилам Google (RFC 9309): берётся группа с самым точным
/// User-agent (иначе `*`), из её правил — самое длинное совпавшее; при равной длине — Allow.
/// `*` — любая последовательность, `$` в конце — конец адреса. Своя реализация: библиотека
/// robotstxt 0.3 не находила совпадение `*.css` с `style.min.css` (повтор символа после `*`).
pub fn allowed(body: &str, agent: &str, url: &str) -> bool {
    let path = match url::Url::parse(url) {
        Ok(u) => format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default()),
        Err(_) => url.to_string(),
    };
    let agent = agent.to_ascii_lowercase();
    // Группы: подряд идущие User-agent делят правила.
    let mut groups: Vec<Group> = Vec::new();
    let mut in_agents = false;
    for raw in body.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        let Some((key, val)) = line.split_once(':') else { continue };
        let (key, val) = (key.trim().to_ascii_lowercase(), val.trim());
        match key.as_str() {
            "user-agent" => {
                if !in_agents || groups.is_empty() {
                    groups.push((Vec::new(), Vec::new()));
                }
                in_agents = true;
                groups.last_mut().unwrap().0.push(val.to_ascii_lowercase());
            }
            "allow" | "disallow" => {
                in_agents = false;
                if let Some(g) = groups.last_mut()
                    && !val.is_empty()
                {
                    g.1.push((key == "allow", val.to_string()));
                }
            }
            _ => {}
        }
    }
    // Самый точный User-agent: совпадение с началом имени бота («googlebot» для «googlebot-image»).
    let score = |g: &Group| {
        g.0.iter().filter(|t| *t != "*" && agent.starts_with(t.as_str())).map(|t| t.len()).max()
    };
    let best = groups.iter().filter_map(score).max();
    let rules: Vec<&(bool, String)> = match best {
        Some(len) => groups.iter().filter(|g| score(g) == Some(len)).flat_map(|g| g.1.iter()).collect(),
        None => groups.iter().filter(|g| g.0.iter().any(|t| t == "*")).flat_map(|g| g.1.iter()).collect(),
    };
    let mut verdict: Option<(usize, bool)> = None;
    for (allow, pattern) in rules {
        if pattern_matches(pattern, &path) {
            let len = pattern.len();
            verdict = match verdict {
                Some((l, a)) if l > len || (l == len && a) => Some((l, a)),
                _ => Some((len, *allow)),
            };
        }
    }
    verdict.is_none_or(|(_, a)| a)
}

/// Совпадение шаблона robots.txt с началом пути: `*` — что угодно, `$` в конце — конец пути.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (pat, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = path.chars().collect();
    // Динамика по позициям: reach[j] — шаблон до i совпал с первыми j символами пути.
    let mut reach = vec![false; t.len() + 1];
    reach[0] = true;
    for &c in &p {
        let mut next = vec![false; t.len() + 1];
        if c == '*' {
            let mut any = false;
            for j in 0..=t.len() {
                any |= reach[j];
                next[j] = any;
            }
        } else {
            for j in 0..t.len() {
                if reach[j] && t[j] == c {
                    next[j + 1] = true;
                }
            }
        }
        reach = next;
    }
    if anchored { reach[t.len()] } else { reach.iter().any(|&r| r) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROBOTS: &str = "User-agent: *\nDisallow: /admin/\nAllow: /admin/public$\n\nUser-agent: GPTBot\nUser-agent: CCBot\nDisallow: /\n\nUser-agent: YandexAdditional\nDisallow: /\n\nHost: example.ru\nClean-param: utm_source /\nSitemap: https://example.ru/sitemap.xml\n";

    #[test]
    fn parses_groups_and_directives() {
        let d = parse(ROBOTS);
        assert_eq!(d.groups.len(), 3);
        assert_eq!(d.groups[1].agents, vec!["gptbot", "ccbot"]);
        assert!(d.groups[1].disallow_all);
        assert!(d.explicit_group("YandexAdditional").unwrap().disallow_all);
        assert!(d.explicit_group("OAI-SearchBot").is_none());
        assert!(d.host && d.clean_param && !d.crawl_delay);
        assert_eq!(d.sitemaps, vec!["https://example.ru/sitemap.xml"]);
    }

    #[test]
    fn google_semantics() {
        assert!(allowed(ROBOTS, "Googlebot", "https://example.ru/"));
        assert!(!allowed(ROBOTS, "Googlebot", "https://example.ru/admin/x"));
        assert!(allowed(ROBOTS, "Googlebot", "https://example.ru/admin/public"));
        assert!(!allowed(ROBOTS, "GPTBot", "https://example.ru/"));
        assert!(allowed(ROBOTS, "OAI-SearchBot", "https://example.ru/"));
    }
}

#[cfg(test)]
mod aigenis_tests {
    use super::*;

    /// robots.txt aigenis.by: «Disallow: /*?» закрывает только адреса с параметрами, не главную.
    const AIGENIS: &str = "User-agent: *\nDisallow: /author/\nDisallow: /cases/obzor-rynka-\nDisallow: /tag/\nDisallow: /policy/\nDisallow: /all-news/\nDisallow: /today-on-the-exchange/\nDisallow: /?s=*\nDisallow: /*?\nDisallow: /*?tpclid=\nDisallow: /glossarium/*/\nDisallow: /wp-json/\nDisallow: /wp-admin/\nDisallow: /wp-includes/\n\nAllow: /wp-content/**/*.css\nAllow: /wp-content/**/*.js\n\nSitemap: https://aigenis.by/sitemap.xml\n";

    #[test]
    fn query_rule_does_not_close_home() {
        for agent in ["Googlebot", "Yandex", "OAI-SearchBot"] {
            assert!(allowed(AIGENIS, agent, "https://aigenis.by/"), "{agent}: главная");
            assert!(allowed(AIGENIS, agent, "https://aigenis.by/uslugi/"), "{agent}: раздел");
            assert!(!allowed(AIGENIS, agent, "https://aigenis.by/?s=test"), "{agent}: поиск закрыт");
            // Ссылка из карточки с меткой закрыта правилом «/*?», а сама главная — нет.
            assert!(!allowed(AIGENIS, agent, "https://aigenis.by/?utm_source=fb"));
            assert!(allowed(AIGENIS, agent, &page_url("https://aigenis.by/?utm_source=fb")));
        }
        let d = parse(AIGENIS);
        assert!(!d.groups[0].disallow_all, "«Disallow: /*?» — не закрытие всего сайта");
    }
}

#[cfg(test)]
mod matcher_tests {
    use super::*;

    #[test]
    fn google_longest_match_and_wildcards() {
        // Случаи из проверки скана: Allow длиннее Disallow — CSS и JS открыты.
        let bitrix = "User-agent: *\nDisallow: /bitrix/\nAllow: /bitrix/*.js\nAllow: /bitrix/*.css\n";
        assert!(allowed(bitrix, "Googlebot", "https://x.by/bitrix/js/core.min.js"));
        assert!(allowed(bitrix, "Googlebot", "https://x.by/bitrix/templates/a/style.min.css"));
        assert!(!allowed(bitrix, "Googlebot", "https://x.by/bitrix/admin/"));
        let wp = "User-agent: GoogleBot\nDisallow: /wp-\nAllow: /*/*.css\nAllow: /*/*.js\n";
        assert!(allowed(wp, "Googlebot", "https://x.by/wp-content/themes/a/style.min.css"));
        assert!(!allowed(wp, "Googlebot", "https://x.by/wp-admin/"));
        // Disallow по расширению срабатывает и при повторе точки в имени.
        assert!(!allowed("User-agent: *\nDisallow: /*.css\n", "Googlebot", "https://x.by/a/style.min.css"));
        // $ — конец адреса.
        let end = "User-agent: *\nDisallow: /*.php$\n";
        assert!(!allowed(end, "Yandex", "https://x.by/index.php"));
        assert!(allowed(end, "Yandex", "https://x.by/index.php?x=1"));
        // Самый точный User-agent; пустой Disallow ничего не закрывает.
        let groups = "User-agent: *\nDisallow: /\n\nUser-agent: Googlebot\nDisallow:\n";
        assert!(allowed(groups, "Googlebot", "https://x.by/"));
        assert!(!allowed(groups, "Yandex", "https://x.by/"));
        // Равная длина — побеждает Allow.
        assert!(allowed("User-agent: *\nDisallow: /page\nAllow: /page\n", "Googlebot", "https://x.by/page"));
    }
}
