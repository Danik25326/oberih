/// Модульна система Oberih.
/// import "path/to/file.obh" — підключає інший файл.
///
/// Особливості:
/// - Шлях відносний до файлу що імпортує
/// - Циклічні імпорти виявляються і ігноруються
/// - Всі функції з імпортованих файлів доступні в поточному

use std::path::{Path, PathBuf};
use std::collections::HashSet;
use crate::parser::ast::{Program, Item, ImportDecl};
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

/// Розгортає всі import декларації в програмі.
/// Повертає нову програму з усіма items з усіх імпортованих файлів.
pub fn resolve_imports(
    program: Program,
    base_path: &Path,
) -> Result<Program, ModuleError> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let canonical = base_path.canonicalize()
        .unwrap_or_else(|_| base_path.to_path_buf());
    visited.insert(canonical);

    resolve_recursive(program, base_path, &mut visited)
}

fn resolve_recursive(
    program: Program,
    base_path: &Path,
    visited:  &mut HashSet<PathBuf>,
) -> Result<Program, ModuleError> {
    let base_dir = base_path.parent()
        .unwrap_or_else(|| Path::new("."));

    let mut all_items: Vec<Item> = Vec::new();

    for item in program.items {
        match &item {
            Item::Import(ImportDecl { path, .. }) => {
                // Вирішуємо шлях відносно поточного файлу
                let import_path = resolve_path(base_dir, path);

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
                all_items.push(item);
            }
        }
    }

    Ok(Program { items: all_items })
}

fn resolve_path(base_dir: &Path, import_path: &str) -> PathBuf {
    let p = Path::new(import_path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}
