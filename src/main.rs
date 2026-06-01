//! Jo compiler driver.
//!
//! Usage: `cargo run -- <source.jo>`
//!
//! Runs preprocessor → lexer → parser → type checker → code generator, writes
//! a `.asm` file, then invokes `nasm` and `ld` to produce an ELF executable
//! named after the source file (no extension) in the current directory.

mod ast;
mod codegen;
mod error;
mod lexer;
mod parser;
mod preprocessor;
mod typechecker;
mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use ast::Program;
use error::{report_all, source_line_of};

use crate::error::Diagnostic;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!(
            "usage: {} <source.jo>",
            args.first().map(String::as_str).unwrap_or("jo_compiler")
        );
        std::process::exit(2);
    }
    let source_path = PathBuf::from(&args[1]);
    match compile(&source_path) {
        Ok(exe) => {
            eprintln!("compiled: {}", exe.display());
        }
        Err(code) => std::process::exit(code),
    }
}

/// Returns the produced executable path on success, or an exit code on failure.
fn compile(source_path: &Path) -> Result<PathBuf, i32> {
    let file_name = source_path.to_string_lossy().to_string();
    let src = match std::fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read `{}`: {}", file_name, e);
            return Err(1);
        }
    };

    // Directories searched (in order) to resolve `module` → `module.jo`:
    //   1. the source file's own directory,
    //   2. the current working directory,
    //   3. the compiler's crate root (where the bundled `stdlib.jo` lives).
    let include_dirs = include_dirs_for(source_path);

    // --- preprocess --------------------------------------------------------
    let pp = match preprocessor::Preprocessor::run(&src, file_name.clone(), &include_dirs) {
        Ok(out) => out,
        Err(diags) => {
            report_all(&diags);
            return Err(1);
        }
    };

    // Preprocess + parse each loaded module (for `module::name` resolution).
    let mut module_asts: HashMap<String, Program> = HashMap::new();
    for (mod_name, mod_src) in &pp.modules {
        match lex_and_parse(mod_src, &format!("{}.jo", mod_name)) {
            Ok(prog) => {
                module_asts.insert(mod_name.clone(), prog);
            }
            Err(diags) => {
                report_all(&diags);
                return Err(1);
            }
        }
    }

    // --- lex + parse main --------------------------------------------------
    let mut program = match lex_and_parse(&pp.source, &file_name) {
        Ok(p) => p,
        Err(diags) => {
            report_all(&diags);
            return Err(1);
        }
    };

    // --- type check --------------------------------------------------------
    if let Err(diags) = typechecker::TypeChecker::check(
        &mut program,
        file_name.clone(),
        pp.source.clone(),
        &mut module_asts,
    ) {
        report_all(&diags);
        return Err(1);
    }

    let mut diags: Vec<Diagnostic> = vec![];

    // Merge type-checked module items (from `import module;`) into the program
    // so the code generator emits their structs' methods and knows their
    // layouts. Glob/named imports already pasted their text, so we skip any
    // struct/function whose name is already present to avoid duplicate labels.
    let mut existing: std::collections::HashSet<String> = std::collections::HashSet::new();

    for i in program.items.iter() {
        match i {
            ast::Item::Struct(s) => existing.insert(s.name.clone()),
            ast::Item::Fn(f) => existing.insert(f.name.clone()),
            ast::Item::Import(_) => false,
            ast::Item::Extend(e) => {
                for f in e.methods.iter() {
                    existing.insert(f.name.clone());
                }

                false
            }
        };
    }

    for (mod_name, mod_prog) in module_asts {
        for item in mod_prog.items {
            if let ast::Item::Extend(e) = item.clone() {
                for f in &e.methods {
                    if existing.insert(f.name.clone()) {
                        program.items.push(ast::Item::Fn(f.clone()));
                    } else {
                        diags.push(duplicate_import_diag(&mod_name, &f.name, f.span, "method"));
                    }
                }
            }

            let name = match &item {
                ast::Item::Struct(s) => s.name.clone(),
                ast::Item::Fn(f) => f.name.clone(),
                ast::Item::Import(_) => continue,
                ast::Item::Extend(_) => continue,
            };
            if existing.insert(name.clone()) {
                program.items.push(item.clone());
            } else {
                let (kind, span) = item_kind_span(&item);
                diags.push(duplicate_import_diag(&mod_name, &name, span, kind));
            }
        }
    }

    report_all(&diags);
    if !diags.is_empty() {
        return Err(1);
    }

    // --- codegen -----------------------------------------------------------
    let asm = codegen::CodeGen::emit(&program);

    // Output paths: executable & asm next to the cwd, named after the source.
    let stem = source_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "a".to_string());
    let asm_path = PathBuf::from(format!("{}.asm", stem));
    let obj_path = PathBuf::from(format!("{}.o", stem));
    let exe_path = PathBuf::from(&stem);

    if let Err(e) = std::fs::write(&asm_path, &asm) {
        eprintln!("error: cannot write `{}`: {}", asm_path.display(), e);
        return Err(1);
    }

    // --- assemble (nasm) ---------------------------------------------------
    let nasm_status = Command::new("nasm")
        .args(["-f", "elf64", "-o"])
        .arg(&obj_path)
        .arg(&asm_path)
        .status();
    match nasm_status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("error: nasm failed with status {}", s);
            return Err(1);
        }
        Err(e) => {
            eprintln!("error: failed to run nasm: {}", e);
            return Err(1);
        }
    }

    // --- link (ld) ---------------------------------------------------------
    let ld_status = Command::new("ld")
        .arg("-o")
        .arg(&exe_path)
        .arg(&obj_path)
        .status();
    match ld_status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("error: ld failed with status {}", s);
            return Err(1);
        }
        Err(e) => {
            eprintln!("error: failed to run ld: {}", e);
            return Err(1);
        }
    }

    Ok(exe_path)
}

fn item_kind_span(item: &ast::Item) -> (&'static str, ast::Span) {
    match item {
        ast::Item::Struct(s) => ("struct", s.span),
        ast::Item::Fn(f) => ("function", f.span),
        ast::Item::Extend(e) => ("extend", e.span),
        ast::Item::Import(_) => ("import", ast::Span::default()),
    }
}

fn duplicate_import_diag(module: &str, name: &str, span: ast::Span, kind: &str) -> Diagnostic {
    let file = format!("{}.jo", module);
    Diagnostic::new(
        "E4E0",
        format!(
            "duplicate {} `{}` imported from module `{}`",
            kind, name, module
        ),
        file.clone(),
        span.line,
        span.col,
        source_line_of("", span.line),
    )
    .with_label("duplicate imported item")
}

fn lex_and_parse(src: &str, file: &str) -> Result<Program, Vec<error::Diagnostic>> {
    let tokens = lexer::Lexer::new(src, file).tokenize()?;
    parser::Parser::new(tokens, file, src).parse()
}

/// Build the ordered list of directories in which `module.jo` files are
/// searched: the source directory, the cwd, and the crate root (baked in at
/// build time so the bundled `stdlib.jo` is always reachable).
fn include_dirs_for(source_path: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(parent) = source_path.parent() {
        let p = if parent.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            parent.to_path_buf()
        };
        dirs.push(p);
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    // De-duplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}
