/// Маніфест проєкту `oberih.toml`.
///
/// Це НЕ повний TOML — лише підмножина, якої досить для нашої схеми:
///   [package]
///   name    = "my-app"
///   version = "0.1.0"
///   entry   = "src/main.obh"
///
///   [dependencies]
///   mathutils    = { path = "../mathutils" }
///   http-helpers = { git = "https://github.com/user/http-helpers.git", branch = "main" }
///
/// Підтримується: рядкові секції `[...]`, `ключ = "рядок"`, інлайн-таблиці
/// `{ k = "v", k2 = "v2" }`, коментарі `#...`. НЕ підтримується: масиви,
/// багаторядкові рядки, вкладені секції, числа/булеві значення як ключі
/// маніфесту (усе, що нам треба, — рядки).
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum DepSource {
    Path(String),
    Git { url: String, branch: Option<String>, tag: Option<String>, rev: Option<String> },
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub name:   String,
    pub source: DepSource,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub name:         String,
    pub version:      String,
    pub entry:        String,
    pub dependencies: Vec<Dependency>,
    /// Директорія, що містить сам `oberih.toml` — інші шляхи в ньому відносні до неї.
    pub root:         PathBuf,
}

pub const MANIFEST_FILE: &str = "oberih.toml";

/// Шукає `oberih.toml` у `start` і вгору по батьківських директоріях (як `Cargo.toml`
/// у Cargo). Повертає шлях до знайденого файлу, якщо є.
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() { start.to_path_buf() } else { start.parent()?.to_path_buf() };
    loop {
        let candidate = dir.join(MANIFEST_FILE);
        if candidate.is_file() { return Some(candidate); }
        if !dir.pop() { return None; }
    }
}

// ---------------------------------------------------------------------------
// Парсер
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ManifestError(pub String);

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "oberih.toml: {}", self.0)
    }
}

type MR<T> = Result<T, ManifestError>;

fn merr(msg: impl Into<String>) -> ManifestError { ManifestError(msg.into()) }

/// Значення інлайн-таблиці: рядок або сама інлайн-таблиця виду `{ k = "v", ... }`.
enum Value {
    Str(String),
    Table(Vec<(String, String)>),
}

/// Знімає екранування лапок і бекслешів у простому подвійнокавичковому рядку.
fn parse_quoted(s: &str) -> MR<String> {
    let s = s.trim();
    if s.len() < 2 || !s.starts_with('"') || !s.ends_with('"') {
        return Err(merr(format!("очікувався рядок у лапках, отримано `{}`", s)));
    }
    let inner = &s[1..s.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"')  => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n')  => out.push('\n'),
                Some(other) => return Err(merr(format!("невідома escape-послідовність \\{}", other))),
                None => return Err(merr("рядок обривається на `\\`")),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn parse_value(raw: &str) -> MR<Value> {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let mut pairs = Vec::new();
        for field in split_top_level(inner, ',') {
            let field = field.trim();
            if field.is_empty() { continue; }
            let (k, v) = field.split_once('=')
                .ok_or_else(|| merr(format!("очікувалось `ключ = значення` в `{}`", field)))?;
            pairs.push((k.trim().to_string(), parse_quoted(v)?));
        }
        Ok(Value::Table(pairs))
    } else {
        Ok(Value::Str(parse_quoted(raw)?))
    }
}

/// Розбиває по роздільнику, ігноруючи той, що всередині лапок.
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' { in_str = !in_str; cur.push(c); }
        else if c == '\\' && in_str { cur.push(c); if let Some(n) = chars.next() { cur.push(n); } }
        else if c == sep && !in_str { parts.push(std::mem::take(&mut cur)); }
        else { cur.push(c); }
    }
    if !cur.trim().is_empty() { parts.push(cur); }
    parts
}

fn table_get(t: &[(String, String)], key: &str) -> Option<String> {
    t.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// Ім'я залежності стає КОМПОНЕНТОМ ШЛЯХУ (`.oberih/deps/<ім'я>`), тому дозволені лише безпечні
/// символи: інакше `../../x` чи `/abs` виводили б клонування `git` за межі кешу (path traversal).
pub fn valid_dep_name(n: &str) -> bool {
    let first_ok = n.chars().next().map_or(false, |c| c.is_ascii_alphanumeric() || c == '_');
    first_ok
        && n.len() <= 64
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !n.contains("..")
}

/// Значення, що потрапляють в аргументи `git`: не можуть починатись з `-` (інакше це опція git,
/// а не URL/гілка/тег/ревізія) і не можуть містити керувальні символи.
fn valid_git_arg(v: &str) -> bool {
    !v.is_empty() && !v.starts_with('-') && !v.chars().any(|c| c.is_control())
}

/// Перевірка залежності незалежно від джерела (розбір файлу або `oberih add`).
pub fn validate_dependency(d: &Dependency) -> MR<()> {
    if !valid_dep_name(&d.name) {
        return Err(merr(format!(
            "недопустиме ім'я залежності `{}`: лише літери/цифри/`_`/`-`/`.`, до 64 символів, без `..` і роздільників шляху",
            d.name.chars().take(40).collect::<String>().escape_debug()
        )));
    }
    if let DepSource::Git { url, branch, tag, rev } = &d.source {
        for (what, val) in [("url", Some(url)), ("branch", branch.as_ref()), ("tag", tag.as_ref()), ("rev", rev.as_ref())] {
            if let Some(v) = val {
                if !valid_git_arg(v) {
                    return Err(merr(format!("залежність '{}': недопустиме значення `{}` (не може починатись з `-` чи містити керувальні символи)", d.name, what)));
                }
            }
        }
    }
    Ok(())
}

fn dep_from_table(name: &str, t: Vec<(String, String)>) -> MR<Dependency> {
    let has_path = table_get(&t, "path");
    let has_git  = table_get(&t, "git");
    let source = match (has_path, has_git) {
        (Some(p), None) => DepSource::Path(p),
        (None, Some(url)) => DepSource::Git {
            url,
            branch: table_get(&t, "branch"),
            tag:    table_get(&t, "tag"),
            rev:    table_get(&t, "rev"),
        },
        (Some(_), Some(_)) => return Err(merr(format!(
            "залежність '{}': вкажіть або `path`, або `git`, не обидва", name
        ))),
        (None, None) => return Err(merr(format!(
            "залежність '{}': потрібне поле `path` або `git` (версії з реєстру не підтримуються — реєстру немає)",
            name
        ))),
    };
    let dep = Dependency { name: name.to_string(), source };
    validate_dependency(&dep)?;
    Ok(dep)
}

pub fn parse_str(text: &str, root: PathBuf) -> MR<Manifest> {
    let mut section = String::new();
    let mut package: Vec<(String, String)> = Vec::new();
    let mut deps: Vec<Dependency> = Vec::new();

    for (lineno, raw_line) in text.lines().enumerate() {
        let line = match raw_line.find('#') { Some(i) => &raw_line[..i], None => raw_line };
        let line = line.trim();
        if line.is_empty() { continue; }
        let err_ctx = |msg: String| merr(format!("рядок {}: {}", lineno + 1, msg));

        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_string();
            if section != "package" && section != "dependencies" {
                return Err(err_ctx(format!("невідома секція `[{}]` (є лише [package] і [dependencies])", section)));
            }
            continue;
        }

        let (key, raw_val) = line.split_once('=')
            .ok_or_else(|| err_ctx(format!("очікувалось `ключ = значення`, отримано `{}`", line)))?;
        let key = key.trim().to_string();
        let value = parse_value(raw_val).map_err(|e| err_ctx(e.0))?;

        match section.as_str() {
            "package" => match value {
                Value::Str(s) => package.push((key, s)),
                Value::Table(_) => return Err(err_ctx(format!("`{}` у [package] має бути рядком", key))),
            },
            "dependencies" => match value {
                Value::Str(_) => return Err(err_ctx(format!(
                    "залежність '{}': потрібна таблиця `{{ path = \"...\" }}` або `{{ git = \"...\" }}`, версії з реєстру не підтримуються",
                    key
                ))),
                Value::Table(t) => deps.push(dep_from_table(&key, t).map_err(|e| err_ctx(e.0))?),
            },
            "" => return Err(err_ctx("значення поза секцією [package]/[dependencies]".into())),
            other => return Err(err_ctx(format!("невідома секція `{}`", other))),
        }
    }

    let name    = table_get(&package, "name").ok_or_else(|| merr("[package]: відсутнє поле `name`"))?;
    let version = table_get(&package, "version").unwrap_or_else(|| "0.1.0".to_string());
    let entry   = table_get(&package, "entry").unwrap_or_else(|| "src/main.obh".to_string());

    Ok(Manifest { name, version, entry, dependencies: deps, root })
}

pub fn load(path: &Path) -> MR<Manifest> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| merr(format!("не вдалось прочитати {}: {}", path.display(), e)))?;
    let root = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    parse_str(&text, root)
}

// ---------------------------------------------------------------------------
// Запис (для `oberih add` і `oberih init`)
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    // parse_quoted розуміє \n, тож перенос рядка треба екранувати: сирий `\n` розірвав би запис
    // (маніфест розбирається построково) і `oberih add` залишав би файл, що не розбирається.
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

impl Manifest {
    pub fn to_toml_string(&self) -> String {
        let mut out = String::new();
        out.push_str("[package]\n");
        out.push_str(&format!("name    = \"{}\"\n", esc(&self.name)));
        out.push_str(&format!("version = \"{}\"\n", esc(&self.version)));
        out.push_str(&format!("entry   = \"{}\"\n", esc(&self.entry)));
        out.push('\n');
        out.push_str("[dependencies]\n");
        for d in &self.dependencies {
            match &d.source {
                DepSource::Path(p) => out.push_str(&format!("{} = {{ path = \"{}\" }}\n", d.name, esc(p))),
                DepSource::Git { url, branch, tag, rev } => {
                    let mut fields = vec![format!("git = \"{}\"", esc(url))];
                    if let Some(b) = branch { fields.push(format!("branch = \"{}\"", esc(b))); }
                    if let Some(t) = tag    { fields.push(format!("tag = \"{}\"", esc(t))); }
                    if let Some(r) = rev    { fields.push(format!("rev = \"{}\"", esc(r))); }
                    out.push_str(&format!("{} = {{ {} }}\n", d.name, fields.join(", ")));
                }
            }
        }
        out
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_toml_string()).map_err(|e| e.to_string())
    }

    /// Додає (або замінює за ім'ям) залежність і повертає новий маніфест.
    pub fn with_dependency(mut self, dep: Dependency) -> Self {
        self.dependencies.retain(|d| d.name != dep.name);
        self.dependencies.push(dep);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(text: &str) -> Manifest { parse_str(text, PathBuf::from(".")).unwrap() }

    #[test]
    fn parses_package_and_deps() {
        let man = m(r#"
# коментар
[package]
name = "demo"
version = "1.2.3"
entry = "src/main.obh"

[dependencies]
mathutils = { path = "../mathutils" }
web = { git = "https://example.com/x.git", branch = "main" }
"#);
        assert_eq!(man.name, "demo");
        assert_eq!(man.version, "1.2.3");
        assert_eq!(man.entry, "src/main.obh");
        assert_eq!(man.dependencies.len(), 2);
        assert_eq!(man.dependencies[0].source, DepSource::Path("../mathutils".into()));
        match &man.dependencies[1].source {
            DepSource::Git { url, branch, .. } => {
                assert_eq!(url, "https://example.com/x.git");
                assert_eq!(branch.as_deref(), Some("main"));
            }
            other => panic!("очікувався Git, отримано {:?}", other),
        }
    }

    #[test]
    fn defaults_version_and_entry() {
        let man = m("[package]\nname = \"x\"\n");
        assert_eq!(man.version, "0.1.0");
        assert_eq!(man.entry, "src/main.obh");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_str("[package]\nversion = \"1\"\n", ".".into()).is_err()); // немає name
        assert!(parse_str("[dependencies]\nx = \"1.0\"\n[package]\nname=\"a\"\n", ".".into()).is_err()); // версія-рядок без path/git
        assert!(parse_str("[dependencies]\nx = {}\n[package]\nname=\"a\"\n", ".".into()).is_err()); // ні path, ні git
        assert!(parse_str("[dependencies]\nx = { path = \"a\", git = \"b\" }\n[package]\nname=\"a\"\n", ".".into()).is_err());
        assert!(parse_str("[nope]\nx = \"1\"\n", ".".into()).is_err());
        assert!(parse_str("garbage line\n", ".".into()).is_err());
    }

    #[test]
    fn roundtrip_write_then_parse() {
        let man = Manifest {
            name: "app".into(), version: "0.1.0".into(), entry: "src/main.obh".into(),
            root: ".".into(),
            dependencies: vec![
                Dependency { name: "a".into(), source: DepSource::Path("../a".into()) },
                Dependency { name: "b".into(), source: DepSource::Git {
                    url: "https://x/y.git".into(), branch: Some("dev".into()), tag: None, rev: None,
                } },
            ],
        };
        let text = man.to_toml_string();
        let back = parse_str(&text, ".".into()).unwrap();
        assert_eq!(back.name, man.name);
        assert_eq!(back.dependencies.len(), 2);
        assert_eq!(back.dependencies[0].source, man.dependencies[0].source);
    }

    #[test]
    fn with_dependency_replaces_by_name() {
        let man = Manifest {
            name: "app".into(), version: "0.1.0".into(), entry: "src/main.obh".into(), root: ".".into(),
            dependencies: vec![Dependency { name: "a".into(), source: DepSource::Path("old".into()) }],
        };
        let man = man.with_dependency(Dependency { name: "a".into(), source: DepSource::Path("new".into()) });
        assert_eq!(man.dependencies.len(), 1);
        assert_eq!(man.dependencies[0].source, DepSource::Path("new".into()));
    }

    #[test]
    fn quoted_strings_with_escapes() {
        let man = m(r#"[package]
name = "has \"quotes\" and \\backslash"
"#);
        assert_eq!(man.name, "has \"quotes\" and \\backslash");
    }

    #[test]
    fn find_manifest_walks_up() {
        let dir = std::env::temp_dir().join(format!("oberih_manifest_test_{}", std::process::id()));
        let sub = dir.join("a/b/c");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(find_manifest(&sub), Some(dir.join(MANIFEST_FILE)));
        assert_eq!(find_manifest(&std::env::temp_dir().join("definitely/not/here/xyz")), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ----- Регресії: знайдено фазером і перевіркою вручну -----

    fn parse_err(text: &str) -> String {
        match parse_str(text, PathBuf::from(".")) { Err(e) => e.0, Ok(_) => panic!("очікувалась помилка для:\n{text}") }
    }

    #[test]
    fn dependency_names_cannot_escape_the_cache_directory() {
        for bad in ["../x", "../../../../tmp/escape", "/etc/x", "a/b", "a\\b", "..", ".hidden", "a..b", "", "x y", "a\"b", &"n".repeat(65)] {
            let text = format!("[package]\nname = \"a\"\n[dependencies]\n{} = {{ path = \"p\" }}\n", bad);
            let e = parse_err(&text);
            assert!(e.contains("ім'я") || e.contains("очікувалось"), "ім'я `{bad}` мало б відхилятись, помилка: {e}");
        }
        // Контроль: нормальні імена проходять.
        for good in ["mathutils", "my-lib_2", "v1.2", "_x", "A9"] {
            let text = format!("[package]\nname = \"a\"\n[dependencies]\n{} = {{ path = \"p\" }}\n", good);
            assert_eq!(parse_str(&text, PathBuf::from(".")).unwrap().dependencies[0].name, good);
        }
    }

    #[test]
    fn git_arguments_cannot_be_options() {
        let e = parse_err("[package]\nname=\"a\"\n[dependencies]\nx = { git = \"--upload-pack=touch /tmp/p\" }\n");
        assert!(e.contains("недопустиме значення `url`"), "{e}");
        for field in ["branch", "tag", "rev"] {
            let e = parse_err(&format!("[package]\nname=\"a\"\n[dependencies]\nx = {{ git = \"https://h/r.git\", {field} = \"-x\" }}\n"));
            assert!(e.contains(&format!("`{field}`")), "{e}");
        }
        // Контроль: звичайний git-URL і гілка проходять.
        assert!(parse_str("[package]\nname=\"a\"\n[dependencies]\nx = { git = \"https://h/r.git\", branch = \"main\" }\n", PathBuf::from(".")).is_ok());
    }

    #[test]
    fn newline_in_values_survives_save_and_reload() {
        // Фазер: шлях із `\n` записувався сирим переносом рядка, і файл переставав розбиратись.
        let man = m("[package]\nname = \"a\"\n[dependencies]\nlib = { path = \"a\\nb\" }\n");
        assert_eq!(man.dependencies[0].source, DepSource::Path("a\nb".into()));
        let again = parse_str(&man.to_toml_string(), PathBuf::from(".")).expect("збережений маніфест має розбиратись");
        assert_eq!(again.dependencies[0].source, DepSource::Path("a\nb".into()));
    }
}
