//! Deterministic compilation from a multi-file console project to one cart.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use console_core::Cart;
use serde::{Deserialize, Serialize};

use crate::palette::{ReportFormat, parse_report_format, resolve_report_format};

mod lua;

pub const BUILD_USAGE: &str = "console build <project|console.toml> [-o|--out out.cart] [--check] [--format text|pretty|json]";

const MANIFEST_NAME: &str = "console.toml";
const DEFAULT_OUTPUT: &str = "build/game.cart";
const CANONICAL_SECTIONS: &[&str] = &[
    "meta",
    "lua",
    "sprites",
    "map",
    "gfx_meta",
    "instruments",
    "sfx",
    "music",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub manifest_version: u32,
    pub cart: CartConfig,
    pub lua: LuaConfig,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub sections: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CartConfig {
    pub title: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub preview_palette: Option<Vec<u8>>,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LuaConfig {
    pub entry: PathBuf,
    #[serde(default = "default_lua_root")]
    pub root: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuildConfig {
    #[serde(default = "default_output")]
    pub output: PathBuf,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            output: default_output(),
        }
    }
}

fn default_lua_root() -> PathBuf {
    PathBuf::from("lua")
}

fn default_output() -> PathBuf {
    PathBuf::from(DEFAULT_OUTPUT)
}

#[derive(Debug)]
pub struct CompiledProject {
    pub cart_text: String,
    pub manifest_path: PathBuf,
    pub project_root: PathBuf,
    pub default_output: PathBuf,
    pub inputs: Vec<PathBuf>,
    pub lua_sources: Vec<LuaSourceMap>,
    pub content_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LuaSourceMap {
    pub module: String,
    pub source: PathBuf,
    pub source_start_line: usize,
    pub source_end_line: usize,
    pub generated_start_line: usize,
    pub generated_end_line: usize,
}

#[derive(Debug, Serialize)]
struct BuildReport {
    command: &'static str,
    project: String,
    manifest: String,
    output: String,
    status: &'static str,
    bytes: usize,
    content_id: String,
    inputs: Vec<String>,
    lua_sources: Vec<LuaSourceMap>,
}

pub fn cli_build(args: &[String]) -> i32 {
    if super::help_requested(args) {
        println!("{BUILD_USAGE}");
        return 0;
    }

    let options = match parse_build_args(args) {
        Ok(options) => options,
        Err(error) => return usage_error(error),
    };
    let compiled = match compile_project(&options.project) {
        Ok(compiled) => compiled,
        Err(error) => return build_error(error),
    };
    let output = options
        .output
        .unwrap_or_else(|| compiled.default_output.clone());

    let status = if options.check {
        match std::fs::read(&output) {
            Ok(current) if current == compiled.cart_text.as_bytes() => "current",
            Ok(_) => {
                return build_error(format!(
                    "{} is stale; run console build to regenerate it",
                    output.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return build_error(format!(
                    "{} does not exist; run console build to create it",
                    output.display()
                ));
            }
            Err(error) => {
                return build_error(format!("cannot read {}: {error}", output.display()));
            }
        }
    } else {
        if let Err(error) = atomic_write(&output, compiled.cart_text.as_bytes()) {
            return build_error(error);
        }
        "written"
    };

    let report = BuildReport {
        command: "console build",
        project: compiled.project_root.display().to_string(),
        manifest: compiled.manifest_path.display().to_string(),
        output: output.display().to_string(),
        status,
        bytes: compiled.cart_text.len(),
        content_id: compiled.content_id,
        inputs: compiled
            .inputs
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        lua_sources: compiled.lua_sources,
    };
    print_build_report(options.format, &report);
    0
}

struct BuildOptions {
    project: PathBuf,
    output: Option<PathBuf>,
    check: bool,
    format: ReportFormat,
}

fn parse_build_args(args: &[String]) -> Result<BuildOptions, String> {
    let mut project = None;
    let mut output = None;
    let mut check = false;
    let mut format = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--out" => {
                index += 1;
                let value = args.get(index).ok_or("--out requires a path")?;
                output = Some(PathBuf::from(value));
            }
            "--check" => check = true,
            "--format" => {
                index += 1;
                let value = args.get(index).ok_or("--format requires a value")?;
                format = Some(parse_report_format(value, "--format")?);
            }
            "--json" => format = Some(ReportFormat::Json),
            flag if flag.starts_with('-') => {
                return Err(format!("unknown console build flag {flag:?}"));
            }
            value if project.is_none() => project = Some(PathBuf::from(value)),
            value => return Err(format!("unexpected argument {value:?}")),
        }
        index += 1;
    }
    let project = project.ok_or("console build requires <project|console.toml>")?;
    Ok(BuildOptions {
        project,
        output,
        check,
        format: resolve_report_format(format)?,
    })
}

pub fn compile_project(input: &Path) -> Result<CompiledProject, String> {
    let (manifest_path, project_root) = discover_project(input)?;
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = toml::from_str(&manifest_text)
        .map_err(|error| format!("invalid {}: {error}", manifest_path.display()))?;
    validate_manifest(&manifest)?;

    let mut inputs = vec![manifest_path.clone()];
    let mut sections = BTreeMap::<String, String>::new();
    sections.insert("meta".into(), render_meta(&manifest.cart)?);

    let lua = lua::bundle(&project_root, &manifest.lua)?;
    inputs.extend(lua.inputs);
    let lua_sources = lua.sources;
    sections.insert("lua".into(), lua.source);

    for (name, relative) in &manifest.sections {
        validate_section_name(name)?;
        if matches!(name.as_str(), "meta" | "lua") {
            return Err(format!(
                "section {name:?} is generated from its manifest table and cannot appear in [sections]"
            ));
        }
        let path = resolve_input(&project_root, relative, &format!("sections.{name}"))?;
        let body = read_source(&path, &format!("section {name}"))?;
        reject_section_markers(&body, &path)?;
        inputs.push(path);
        sections.insert(name.clone(), body);
    }

    inputs.sort();
    inputs.dedup();
    let cart_text = render_cart(&sections);
    Cart::parse(&cart_text).map_err(|error| format!("generated cart is invalid: {error}"))?;

    let default_output = resolve_output(&project_root, &manifest.build.output)?;
    let content_id = format!("fnv1a64:{:016x}", fnv1a64(cart_text.as_bytes()));
    Ok(CompiledProject {
        cart_text,
        manifest_path,
        project_root,
        default_output,
        inputs,
        lua_sources,
        content_id,
    })
}

fn discover_project(input: &Path) -> Result<(PathBuf, PathBuf), String> {
    let input = std::fs::canonicalize(input)
        .map_err(|error| format!("cannot open project {}: {error}", input.display()))?;
    let (manifest, root) = if input.is_dir() {
        (input.join(MANIFEST_NAME), input)
    } else {
        if input.file_name().and_then(|name| name.to_str()) != Some(MANIFEST_NAME) {
            return Err(format!(
                "project manifest must be named {MANIFEST_NAME}, got {}",
                input.display()
            ));
        }
        let root = input
            .parent()
            .ok_or_else(|| format!("{} has no project directory", input.display()))?
            .to_path_buf();
        (input, root)
    };
    let manifest = std::fs::canonicalize(&manifest)
        .map_err(|error| format!("cannot open {}: {error}", manifest.display()))?;
    if !manifest.starts_with(&root) {
        return Err(format!(
            "project manifest {} escapes project root {}",
            manifest.display(),
            root.display()
        ));
    }
    Ok((manifest, root))
}

fn validate_manifest(manifest: &Manifest) -> Result<(), String> {
    if manifest.manifest_version != 1 {
        return Err(format!(
            "unsupported manifest_version {}; expected 1",
            manifest.manifest_version
        ));
    }
    validate_meta_value("cart.title", &manifest.cart.title)?;
    if manifest.cart.title.trim().is_empty() {
        return Err("cart.title cannot be empty".into());
    }
    if let Some(author) = &manifest.cart.author {
        validate_meta_value("cart.author", author)?;
    }
    if let Some(version) = &manifest.cart.version {
        validate_meta_value("cart.version", version)?;
    }
    for (key, value) in &manifest.cart.meta {
        validate_meta_key(key)?;
        validate_meta_value(&format!("cart.meta.{key}"), value)?;
    }
    if manifest.lua.root.as_os_str().is_empty() {
        return Err("lua.root cannot be empty".into());
    }
    validate_relative(&manifest.lua.root, "lua.root")?;
    Ok(())
}

fn render_meta(config: &CartConfig) -> Result<String, String> {
    let mut values = config.meta.clone();
    for reserved in ["title", "author", "version", "preview_palette"] {
        if values.contains_key(reserved) {
            return Err(format!(
                "cart.meta.{reserved} duplicates the typed cart.{reserved} field"
            ));
        }
    }
    values.insert("title".into(), config.title.clone());
    if let Some(author) = &config.author {
        values.insert("author".into(), author.clone());
    }
    if let Some(version) = &config.version {
        values.insert("version".into(), version.clone());
    }
    if let Some(palette) = &config.preview_palette {
        if palette.iter().any(|index| *index > 63) {
            return Err("cart.preview_palette values must be 0-63".into());
        }
        values.insert(
            "preview_palette".into(),
            palette
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    Ok(values
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

pub(super) fn resolve_input(root: &Path, relative: &Path, label: &str) -> Result<PathBuf, String> {
    validate_relative(relative, label)?;
    let path = root.join(relative);
    let resolved = std::fs::canonicalize(&path)
        .map_err(|error| format!("cannot open {label} {}: {error}", path.display()))?;
    if !resolved.starts_with(root) {
        return Err(format!(
            "{label} {} escapes project root {}",
            path.display(),
            root.display()
        ));
    }
    if !resolved.is_file() {
        return Err(format!("{label} {} is not a file", path.display()));
    }
    Ok(resolved)
}

fn resolve_output(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    validate_relative(relative, "build.output")?;
    let output = root.join(relative);
    match std::fs::symlink_metadata(&output) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "build.output {} cannot be a symbolic link",
                output.display()
            ));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!(
                "build.output {} exists but is not a file",
                output.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot inspect build.output {}: {error}",
                output.display()
            ));
        }
    }
    let parent = output
        .parent()
        .ok_or_else(|| "build.output has no parent directory".to_string())?;
    let mut existing = parent;
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| "build.output has no existing ancestor".to_string())?;
    }
    let resolved_parent = std::fs::canonicalize(existing)
        .map_err(|error| format!("cannot resolve build.output parent: {error}"))?;
    if !resolved_parent.starts_with(root) {
        return Err(format!(
            "build.output {} escapes project root {}",
            output.display(),
            root.display()
        ));
    }
    Ok(output)
}

pub(super) fn validate_relative(path: &Path, label: &str) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!("{label} must be a non-empty relative path"));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!("{label} cannot contain parent or root components"));
    }
    Ok(())
}

pub(super) fn read_source(path: &Path, label: &str) -> Result<String, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {label} {} as UTF-8: {error}", path.display()))?;
    Ok(normalize_body(&source))
}

fn normalize_body(source: &str) -> String {
    source
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_string()
}

pub(super) fn reject_section_markers(source: &str, path: &Path) -> Result<(), String> {
    for (line_index, line) in source.lines().enumerate() {
        if section_marker(line).is_some() {
            return Err(format!(
                "{}:{} contains a cart section marker; source files may only contain section bodies",
                path.display(),
                line_index + 1
            ));
        }
    }
    Ok(())
}

fn section_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("__")?.strip_suffix("__")?;
    (!inner.is_empty()
        && inner
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_'))
    .then_some(inner)
}

fn validate_section_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name != name.to_ascii_lowercase()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(format!(
            "invalid section name {name:?}; use lowercase ASCII letters, digits, and underscores"
        ));
    }
    Ok(())
}

fn validate_meta_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(format!(
            "invalid metadata key {key:?}; use ASCII letters, digits, and underscores"
        ));
    }
    Ok(())
}

fn validate_meta_value(label: &str, value: &str) -> Result<(), String> {
    if value.contains(['\r', '\n']) {
        return Err(format!("{label} cannot contain a newline"));
    }
    Ok(())
}

fn render_cart(sections: &BTreeMap<String, String>) -> String {
    let canonical = CANONICAL_SECTIONS.iter().copied().collect::<BTreeSet<_>>();
    let mut order = CANONICAL_SECTIONS
        .iter()
        .filter(|name| sections.contains_key(**name))
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    order.extend(
        sections
            .keys()
            .filter(|name| !canonical.contains(name.as_str()))
            .cloned(),
    );

    let mut output = String::new();
    for name in order {
        output.push_str("__");
        output.push_str(&name);
        output.push_str("__\n");
        output.push_str(&sections[&name]);
        output.push('\n');
    }
    output
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create output directory {}: {error}",
            parent.display()
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("output path {} has no UTF-8 file name", path.display()))?;
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut last_error = None;
    for _ in 0..100 {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            serial
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => {
                let result = write_and_replace(file, &temp, path, bytes);
                if result.is_err() {
                    let _ = std::fs::remove_file(&temp);
                }
                return result;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => {
                return Err(format!(
                    "cannot create temporary output {}: {error}",
                    temp.display()
                ));
            }
        }
    }
    Err(format!(
        "cannot allocate temporary output beside {}: {}",
        path.display(),
        last_error.map_or_else(|| "unknown error".into(), |error| error.to_string())
    ))
}

fn write_and_replace(
    mut file: std::fs::File,
    temp: &Path,
    path: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write temporary output {}: {error}", temp.display()))?;
    drop(file);
    std::fs::rename(temp, path).map_err(|error| {
        format!(
            "cannot atomically replace {} with {}: {error}",
            path.display(),
            temp.display()
        )
    })
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn print_build_report(format: ReportFormat, report: &BuildReport) {
    match format {
        ReportFormat::Json => println!(
            "{}",
            serde_json::to_string(report).expect("build report JSON")
        ),
        ReportFormat::Pretty => {
            println!("console build: {}", report.status);
            println!("  project: {}", report.project);
            println!("  manifest: {}", report.manifest);
            println!("  output:  {}", report.output);
            println!("  cart:    {} bytes ({})", report.bytes, report.content_id);
            println!("  inputs:");
            for input in &report.inputs {
                println!("    {input}");
            }
            println!("  Lua sources:");
            for source in &report.lua_sources {
                println!(
                    "    {}: {} (generated lines {}-{})",
                    source.module,
                    source.source.display(),
                    source.generated_start_line,
                    source.generated_end_line
                );
            }
        }
        ReportFormat::Text => {
            println!("command={}", report.command);
            println!("status={}", report.status);
            println!("project={}", report.project);
            println!("manifest={}", report.manifest);
            println!("output={}", report.output);
            println!("bytes={}", report.bytes);
            println!("content_id={}", report.content_id);
            for input in &report.inputs {
                println!("input={input}");
            }
            for source in &report.lua_sources {
                println!(
                    "lua_source={}|{}|{}-{}",
                    source.module,
                    source.source.display(),
                    source.generated_start_line,
                    source.generated_end_line
                );
            }
        }
    }
}

fn usage_error(error: impl std::fmt::Display) -> i32 {
    eprintln!("error: {error}");
    eprintln!("{BUILD_USAGE}");
    2
}

fn build_error(error: impl std::fmt::Display) -> i32 {
    eprintln!("error: {error}");
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TempProject(PathBuf);

    impl TempProject {
        fn new() -> Self {
            let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "console-project-test-{}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(path.join("lua")).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, body: &str) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, body).unwrap();
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn manifest(extra: &str) -> String {
        format!(
            "manifest_version = 1\n\n[cart]\ntitle = \"Test Game\"\nauthor = \"Agent\"\nversion = \"1\"\n\n[lua]\nentry = \"lua/main.lua\"\n\n{extra}"
        )
    }

    #[test]
    fn compiles_canonical_cart_from_separate_sources() {
        let project = TempProject::new();
        project.write(
            "console.toml",
            &manifest("[sections]\nz_notes = \"notes.txt\"\nmap = \"map.txt\"\nsprites = \"sprites.txt\"\n"),
        );
        project.write("lua/main.lua", "function _draw() end\r\n");
        project.write("map.txt", "");
        project.write("sprites.txt", "");
        project.write("notes.txt", "hello\r\nworld\r\n");

        let first = compile_project(&project.0).unwrap();
        let second = compile_project(&project.0.join("console.toml")).unwrap();
        assert_eq!(first.cart_text, second.cart_text);
        assert_eq!(first.content_id, second.content_id);
        let cart = Cart::parse(&first.cart_text).unwrap();
        assert_eq!(cart.title(), "Test Game");
        assert!(cart.lua().contains("local require"));
        assert!(
            cart.lua()
                .contains("-- console entry\nfunction _draw() end")
        );
        assert_eq!(cart.section("z_notes").unwrap().trim_end(), "hello\nworld");
    }

    #[test]
    fn rejects_unsupported_versions_and_section_injection() {
        let project = TempProject::new();
        project.write("console.toml", &manifest(""));
        project.write("lua/main.lua", "__sprites__\n");
        let error = compile_project(&project.0).unwrap_err();
        assert!(error.contains("contains a cart section marker"), "{error}");

        project.write("lua/main.lua", "function _draw() end\n");
        project.write("console.toml", &manifest("").replacen("1", "2", 1));
        let error = compile_project(&project.0).unwrap_err();
        assert!(error.contains("unsupported manifest_version 2"), "{error}");
    }

    #[test]
    fn rejects_parent_components_and_invalid_generated_carts() {
        let project = TempProject::new();
        project.write(
            "console.toml",
            &manifest("[sections]\nnotes = \"../outside.txt\"\n"),
        );
        project.write("lua/main.lua", "function _draw() end\n");
        let error = compile_project(&project.0).unwrap_err();
        assert!(error.contains("cannot contain parent"), "{error}");

        project.write(
            "console.toml",
            &manifest("[sections]\nsprites = \"sprites.txt\"\n"),
        );
        project.write("sprites.txt", "not pixels\n");
        let error = compile_project(&project.0).unwrap_err();
        assert!(error.contains("generated cart is invalid"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_input_symlinks_that_escape_the_project() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new();
        project.write(
            "console.toml",
            &manifest("[sections]\nnotes = \"notes.txt\"\n"),
        );
        project.write("lua/main.lua", "function _draw() end\n");
        let outside = project.0.with_extension("outside.txt");
        std::fs::write(&outside, "outside").unwrap();
        symlink(&outside, project.0.join("notes.txt")).unwrap();
        let error = compile_project(&project.0).unwrap_err();
        assert!(error.contains("escapes project root"), "{error}");
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn atomic_write_replaces_a_file_without_leaving_temps() {
        let project = TempProject::new();
        let output = project.0.join("nested/game.cart");
        atomic_write(&output, b"first").unwrap();
        atomic_write(&output, b"second").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"second");
        let entries = std::fs::read_dir(output.parent().unwrap())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
    }
}
