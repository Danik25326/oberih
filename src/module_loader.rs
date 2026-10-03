/// Модульна система Oberih.
/// import "path/to/file.obh" — підключає інший файл.
///
/// Особливості:
/// - Шлях відносний до файлу що імпортує
/// - Циклічні імпорти виявляються і ігноруються
/// - Всі функції з імпортованих файлів доступні в поточному

use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use crate::parser::ast::{Program, Item, ImportDecl, Stmt, Expr, MatchArm, LambdaBody};
use crate::lexer::Lexer;
use crate::parser::Parser;

#[derive(Debug)]
pub struct ModuleError {
    pub message: String,
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Помилка модуля: {}", self.message)
    }
}

/// Облік видимості: де визначено кожен top-level fn/struct (і чи він
/// `private`), та які імена згадуються в кожному файлі. Після розгортання
/// імпортів це дозволяє знайти звернення до приватного імені з ІНШОГО файлу.
#[derive(Default)]
struct Visibility {
    /// (ім'я, файл-власник, is_private)
    defs: Vec<(String, PathBuf, bool)>,
    /// (файл, згадане ім'я, рядок, колонка)
    refs: Vec<(PathBuf, String, usize, usize)>,
}

impl Visibility {
    fn check(&self) -> Result<(), ModuleError> {
        for (file, name, line, col) in &self.refs {
            let defined_here = self.defs.iter().any(|(n, f, _)| n == name && f == file);
            if defined_here { continue; }
            let public_elsewhere = self.defs.iter().any(|(n, f, p)| n == name && f != file && !*p);
            if public_elsewhere { continue; }
            if let Some((_, owner, _)) = self.defs.iter().find(|(n, f, p)| n == name && f != file && *p) {
                return Err(ModuleError {
                    message: format!(
                        "{}:{}:{}: '{}' є private у '{}' і недоступна з іншого файлу",
                        file.display(), line, col, name, owner.display()
                    ),
                });
            }
        }
        Ok(())
    }
}

pub(crate) fn collect_stmt_refs(stmt: &Stmt, out: &mut Vec<(String, usize, usize)>) {
    match stmt {
        Stmt::Let { value, .. }  => collect_expr_refs(value, out),
        Stmt::Return { value, .. } => collect_expr_refs(value, out),
        Stmt::If { cond, then_body, else_body, .. } => {
            collect_expr_refs(cond, out);
            for s in then_body { collect_stmt_refs(s, out); }
            if let Some(eb) = else_body { for s in eb { collect_stmt_refs(s, out); } }
        }
        Stmt::While { cond, body, .. } => {
            collect_expr_refs(cond, out);
            for s in body { collect_stmt_refs(s, out); }
        }
        Stmt::For { iter, body, .. } => {
            collect_expr_refs(iter, out);
            for s in body { collect_stmt_refs(s, out); }
        }
        Stmt::Expr(e) => collect_expr_refs(e, out),
        Stmt::Assign { target, value, .. } => {
            collect_expr_refs(target, out);
            collect_expr_refs(value, out);
        }
    }
}

pub(crate) fn collect_expr_refs(expr: &Expr, out: &mut Vec<(String, usize, usize)>) {
    match expr {
        Expr::Number(..) | Expr::StringLit(..) | Expr::Bool(..) => {}
        Expr::Ident(name, sp) => out.push((name.clone(), sp.line, sp.col)),
        Expr::ResultCtor { value, .. } => collect_expr_refs(value, out),
        Expr::BinOp { left, right, .. } => {
            collect_expr_refs(left, out);
            collect_expr_refs(right, out);
        }
        Expr::BitNot { expr, .. } => collect_expr_refs(expr, out),
        Expr::Neg { expr, .. } | Expr::Not { expr, .. } | Expr::Try { expr, .. } => {
            collect_expr_refs(expr, out)
        }
        Expr::Call { callee, args, .. } => {
            collect_expr_refs(callee, out);
            for a in args { collect_expr_refs(a, out); }
        }
        Expr::Field { object, .. } => collect_expr_refs(object, out),
        Expr::MethodCall { object, args, .. } => {
            collect_expr_refs(object, out);
            for a in args { collect_expr_refs(a, out); }
        }
        Expr::Index { object, index, .. } => {
            collect_expr_refs(object, out);
            collect_expr_refs(index, out);
        }
        Expr::Match { scrutinee, arms, .. } => {
            collect_expr_refs(scrutinee, out);
            for MatchArm { body, .. } in arms { collect_expr_refs(body, out); }
        }
        Expr::Spawn { fn_name, args, span } => {
            out.push((fn_name.clone(), span.line, span.col));
            for a in args { collect_expr_refs(a, out); }
        }
        Expr::List(items, _) => for i in items { collect_expr_refs(i, out); },
        Expr::Lambda { body, .. } => match body {
            LambdaBody::Expr(e)   => collect_expr_refs(e, out),
            LambdaBody::Block(bs) => for s in bs { collect_stmt_refs(s, out); },
        },
        Expr::Map(pairs, _) => for (k, v) in pairs {
            collect_expr_refs(k, out);
            collect_expr_refs(v, out);
        },
    }
}

/// Розгортає всі import декларації в програмі.
/// Повертає нову програму з усіма items з усіх імпортованих файлів.
/// Перевіряє, що `private` fn/struct не використовуються з інших файлів.
pub fn resolve_imports(program: Program, base_path: &Path) -> Result<Program, ModuleError> {
    resolve_imports_with_packages(program, base_path, &HashMap::new())
}

/// Як `resolve_imports`, але `import "ім'я/файл.obh"`, де `ім'я` — ключ у
/// `packages` (з `oberih.toml`, після `deps::resolve_all`), розв'язується
/// відносно кореня цього пакета, а не відносно поточного файлу.
pub fn resolve_imports_with_packages(
    program: Program,
    base_path: &Path,
    packages: &HashMap<String, PathBuf>,
) -> Result<Program, ModuleError> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let canonical = base_path.canonicalize()
        .unwrap_or_else(|_| base_path.to_path_buf());
    visited.insert(canonical);

    let mut vis = Visibility::default();
    let result = resolve_recursive(program, base_path, &mut visited, &mut vis, packages)?;
    vis.check()?;
    Ok(result)
}

fn resolve_recursive(
    program: Program,
    base_path: &Path,
    visited:  &mut HashSet<PathBuf>,
    vis:      &mut Visibility,
    packages: &HashMap<String, PathBuf>,
) -> Result<Program, ModuleError> {
    let base_dir = base_path.parent()
        .unwrap_or_else(|| Path::new("."));

    let mut all_items: Vec<Item> = Vec::new();

    for item in program.items {
        match &item {
            Item::Import(ImportDecl { path, .. }) => {
                // Вирішуємо шлях: якщо перший компонент — ім'я залежності з
                // oberih.toml, беремо відносно її кореня; інакше — відносно
                // поточного файлу, як завжди.
                let import_path = resolve_path(base_dir, path, packages);

                // Перевіряємо циклічний імпорт
                let canonical = import_path.canonicalize()
                    .unwrap_or_else(|_| import_path.clone());

                if visited.contains(&canonical) {
                    // Циклічний або повторний імпорт — пропускаємо
                    continue;
                }
                visited.insert(canonical);

                // Читаємо файл
                let src = std::fs::read_to_string(&import_path)
                    .map_err(|e| ModuleError {
                        message: format!(
                            "Не вдалось прочитати '{}': {}",
                            import_path.display(), e
                        ),
                    })?;

                // Парсимо
                let tokens = Lexer::new(&src).tokenize()
                    .map_err(|e| ModuleError {
                        message: format!(
                            "'{}': лексична помилка {}:{}: {}",
                            import_path.display(), e.line, e.col, e.message
                        ),
                    })?;

                let imported_program = Parser::new(tokens).parse_program()
                    .map_err(|e| ModuleError {
                        message: format!(
                            "'{}': синтаксична помилка {}:{}: {}",
                            import_path.display(), e.line, e.col, e.message
                        ),
                    })?;

                // Рекурсивно розгортаємо імпорти в імпортованому файлі
                let resolved = resolve_recursive(
                    imported_program,
                    &import_path,
                    visited,
                    vis,
                    packages,
                )?;

                // Додаємо items (крім fn main — щоб не дублювати)
                for imported_item in resolved.items {
                    match &imported_item {
                        Item::Fn(f) if f.name == "main" => {
                            // main з імпортованого файлу не додаємо
                        }
                        _ => all_items.push(imported_item),
                    }
                }
            }
            _ => {
                // Реєструємо визначення та посилання ЦЬОГО файлу для перевірки private.
                let file = base_path.canonicalize().unwrap_or_else(|_| base_path.to_path_buf());
                match &item {
                    Item::Fn(f) => {
                        // Методи (`Type.method`) не є окремими іменами верхнього рівня.
                        if !f.name.contains('.') {
                            vis.defs.push((f.name.clone(), file.clone(), f.is_private));
                        }
                        let mut found = Vec::new();
                        for st in &f.body { collect_stmt_refs(st, &mut found); }
                        for (n, l, c) in found { vis.refs.push((file.clone(), n, l, c)); }
                    }
                    Item::Struct(st) => {
                        vis.defs.push((st.name.clone(), file.clone(), st.is_private));
                    }
                    _ => {}
                }
                all_items.push(item);
            }
        }
    }

    Ok(Program { items: all_items })
}

fn resolve_path(base_dir: &Path, import_path: &str, packages: &HashMap<String, PathBuf>) -> PathBuf {
    let p = Path::new(import_path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    // "ім'я_пакета/решта/шляху.obh" -> <корінь_пакета>/решта/шляху.obh
    if let Some((first, rest)) = import_path.split_once('/') {
        if let Some(pkg_root) = packages.get(first) {
            return pkg_root.join(rest);
        }
    }
    base_dir.join(p)
}
