/// Розв'язання залежностей з `oberih.toml` у локальні директорії:
/// `path`-залежність — вже локальна, лише перевіряємо, що існує;
/// `git`-залежність — клонується (або оновлюється, якщо вже клонована) у
/// кеш `<корінь_проєкту>/.oberih/deps/<ім'я>` через системну команду `git`.
///
/// Немає власного git-клієнта: git — складний бінарний протокол, обгортати
/// системний `git` набагато надійніше, ніж переписувати його самотужки.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::manifest::{DepSource, Manifest};

#[derive(Debug)]
pub struct DepsError(pub String);

impl std::fmt::Display for DepsError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn derr(msg: impl Into<String>) -> DepsError { DepsError(msg.into()) }

fn cache_dir(project_root: &Path) -> PathBuf {
    project_root.join(".oberih").join("deps")
}

fn run_git(args: &[&str], cwd: &Path, ctx: &str) -> Result<(), DepsError> {
    let out = Command::new("git").args(args).current_dir(cwd).output()
        .map_err(|e| derr(format!(
            "{}: не вдалось запустити `git` ({}) — перевірте, що git встановлений і є в PATH", ctx, e
        )))?;
    if !out.status.success() {
        return Err(derr(format!(
            "{}: git завершився з помилкою:\n{}", ctx, String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Клонує (якщо кешу ще немає) або оновлює git-залежність. `rev`/`tag` фіксують
/// точний стан — оновлення пропускається, якщо кеш вже існує (як lock-файл).
/// Без `rev`/`tag` кеш оновлюється (`git fetch` + `reset --hard`) щоразу.
fn fetch_git(name: &str, url: &str, branch: Option<&str>, tag: Option<&str>, rev: Option<&str>,
             dest: &Path) -> Result<(), DepsError> {
    let pin = tag.or(rev);
    if dest.is_dir() {
        if pin.is_some() {
            return Ok(()); // зафіксована версія — кеш незмінний, оновлювати нема чого
        }
        let branch_ref = branch.unwrap_or("HEAD");
        run_git(&["fetch", "--depth", "1", "origin", branch_ref], dest,
            &format!("залежність '{}': оновлення", name))?;
        run_git(&["reset", "--hard", "FETCH_HEAD"], dest,
            &format!("залежність '{}': оновлення", name))?;
        return Ok(());
    }

    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))
        .map_err(|e| derr(format!("залежність '{}': не вдалось створити кеш: {}", name, e)))?;

    let mut args: Vec<&str> = vec!["clone", "--quiet"];
    if let Some(b) = branch { args.extend(["--branch", b]); }
    if pin.is_none() { args.extend(["--depth", "1"]); } // без фіксованої ревізії — мілке клонування
    let dest_str = dest.to_string_lossy().into_owned();
    args.push(url);
    args.push(&dest_str);
    run_git(&args, Path::new("."), &format!("залежність '{}': клонування", name))?;

    if let Some(r) = rev {
        run_git(&["checkout", "--quiet", r], dest, &format!("залежність '{}': checkout {}", name, r))?;
    }
    Ok(())
}

/// Розв'язує всі залежності маніфесту. Повертає карту ім'я → корінь пакета
/// (директорія, у якій шукати файли цієї залежності).
pub fn resolve_all(manifest: &Manifest) -> Result<HashMap<String, PathBuf>, DepsError> {
    let mut out = HashMap::new();
    for dep in &manifest.dependencies {
        let root = match &dep.source {
            DepSource::Path(p) => {
                let joined = manifest.root.join(p);
                if !joined.is_dir() {
                    return Err(derr(format!(
                        "залежність '{}': шлях '{}' не існує або не є директорією",
                        dep.name, joined.display()
                    )));
                }
                joined.canonicalize().unwrap_or(joined)
            }
            DepSource::Git { url, branch, tag, rev } => {
                let dest = cache_dir(&manifest.root).join(&dep.name);
                fetch_git(&dep.name, url, branch.as_deref(), tag.as_deref(), rev.as_deref(), &dest)?;
                dest
            }
        };
        out.insert(dep.name.clone(), root);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Dependency;

    fn has_git() -> bool {
        Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    #[test]
    fn path_dependency_resolves_to_canonical_dir() {
        let tmp = std::env::temp_dir().join(format!("oberih_deps_path_{}", std::process::id()));
        let lib = tmp.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let manifest = Manifest {
            name: "app".into(), version: "0.1.0".into(), entry: "src/main.obh".into(),
            root: tmp.clone(),
            dependencies: vec![Dependency { name: "lib".into(), source: DepSource::Path("lib".into()) }],
        };
        let resolved = resolve_all(&manifest).unwrap();
        assert_eq!(resolved["lib"], lib.canonicalize().unwrap());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn missing_path_dependency_is_a_clear_error() {
        let manifest = Manifest {
            name: "app".into(), version: "0.1.0".into(), entry: "src/main.obh".into(),
            root: std::env::temp_dir(),
            dependencies: vec![Dependency { name: "ghost".into(), source: DepSource::Path("does-not-exist".into()) }],
        };
        let e = resolve_all(&manifest).unwrap_err();
        assert!(e.0.contains("ghost"), "{}", e.0);
    }

    #[test]
    fn git_dependency_clones_into_cache_and_is_reused() {
        if !has_git() { eprintln!("git не встановлено в цьому середовищі — пропускаю тест"); return; }
        // Локальний git-репозиторій замість мережі — тест детермінований і офлайн.
        let origin = std::env::temp_dir().join(format!("oberih_deps_origin_{}", std::process::id()));
        std::fs::create_dir_all(&origin).unwrap();
        let run = |args: &[&str], cwd: &Path| {
            assert!(Command::new("git").args(args).current_dir(cwd).output().unwrap().status.success(), "{:?}", args);
        };
        run(&["init", "--quiet", "-b", "main"], &origin);
        run(&["config", "user.email", "t@example.com"], &origin);
        run(&["config", "user.name", "t"], &origin);
        std::fs::write(origin.join("lib.obh"), "fn hi() -> Number { return 1 }\n").unwrap();
        run(&["add", "."], &origin);
        run(&["commit", "--quiet", "-m", "init"], &origin);

        let project = std::env::temp_dir().join(format!("oberih_deps_project_{}", std::process::id()));
        std::fs::create_dir_all(&project).unwrap();
        let manifest = Manifest {
            name: "app".into(), version: "0.1.0".into(), entry: "src/main.obh".into(),
            root: project.clone(),
            dependencies: vec![Dependency {
                name: "lib".into(),
                source: DepSource::Git {
                    url: origin.to_string_lossy().into_owned(), branch: Some("main".into()), tag: None, rev: None,
                },
            }],
        };
        let resolved = resolve_all(&manifest).unwrap();
        assert!(resolved["lib"].join("lib.obh").is_file());
        // Повторний виклик (оновлення вже клонованого кешу) не падає.
        assert!(resolve_all(&manifest).is_ok());

        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&project).ok();
    }
}
