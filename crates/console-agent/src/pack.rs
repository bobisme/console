//! Build a cart and the browser runtime into one self-contained HTML page.

use std::fs;
use std::path::{Path, PathBuf};

use console_core::Cart;

pub const USAGE: &str = r#"console pack — bundle a cart into a self-contained HTML file

USAGE:
    console pack <cart> -o <out.html> [OPTIONS]

ARGS:
    <cart>                  Path to the .cart file (UTF-8 text)

OPTIONS:
    -o, --out <FILE>        Output HTML file (required)
        --engine <FILE>     Override the embedded browser engine
        --template <FILE>   Override the embedded HTML template
    -h, --help              Print this help

OUTPUT:
    The result has zero external references and runs directly from file://.
    The default engine and template are embedded in the console executable, so
    this command works from any directory after installation.
"#;

const DEFAULT_ENGINE: &str = include_str!("../../../web/engine.js");
const DEFAULT_TEMPLATE: &str = include_str!("../../../web/template.html");

#[derive(Debug, Clone)]
pub struct BundleOptions {
    pub cart: PathBuf,
    pub engine: Option<PathBuf>,
    pub template: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Bundle {
    pub title: String,
    pub html: String,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
struct PackArgs {
    bundle: BundleOptions,
    out: PathBuf,
}

pub fn cli_pack(args: &[String]) -> i32 {
    let parsed = match parse_args(args) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return 0;
        }
        Err(error) => {
            eprintln!("console pack: error: {error}");
            return 2;
        }
    };

    let bundle = match bundle(&parsed.bundle) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("console pack: error: {error}");
            return 1;
        }
    };
    for warning in &bundle.warnings {
        eprintln!("console pack: warning: {warning}");
    }

    if let Some(dir) = parsed.out.parent()
        && !dir.as_os_str().is_empty()
        && let Err(error) = fs::create_dir_all(dir)
    {
        eprintln!("console pack: error: creating {}: {error}", dir.display());
        return 1;
    }
    if let Err(error) = fs::write(&parsed.out, bundle.html.as_bytes()) {
        eprintln!(
            "console pack: error: writing {}: {error}",
            parsed.out.display()
        );
        return 1;
    }

    eprintln!(
        "console pack: {} ({}) -> {} ({} bytes)",
        parsed.bundle.cart.display(),
        bundle.title,
        parsed.out.display(),
        bundle.html.len()
    );
    0
}

/// Build HTML in memory. `console pack` writes it to disk; `console serve`
/// sends it directly to the browser and calls this again on each refresh.
pub fn bundle(options: &BundleOptions) -> Result<Bundle, String> {
    let cart_text = read_text(&options.cart)?;
    let (engine_js, engine_label) = read_override_or_embedded(
        options.engine.as_deref(),
        DEFAULT_ENGINE,
        "embedded web/engine.js",
    )?;
    let (template, _) = read_override_or_embedded(
        options.template.as_deref(),
        DEFAULT_TEMPLATE,
        "embedded web/template.html",
    )?;

    let cart = Cart::parse(&cart_text).map_err(|e| format!("{}: {e}", options.cart.display()))?;

    if let Some(at) = find_ignore_ascii_case(&engine_js, "</script") {
        return Err(format!(
            "{engine_label}: engine JS contains `</script` at byte offset {at}; it cannot be \
             embedded in a <script> element. Rebuild the engine with web/build-engine.sh."
        ));
    }

    let mut warnings = Vec::new();
    if !engine_js.is_ascii() {
        warnings.push(format!(
            "{engine_label} is not pure ASCII (raw wasm embedding?). The packed HTML may not \
             be safely text-editable. Rebuild with -sSINGLE_FILE_BINARY_ENCODE=0."
        ));
    }

    let title = cart.title().to_owned();
    let html = render(
        &template,
        &[
            ("TITLE", &html_escape(&title)),
            ("CART_TEXT", &escape_cart_text(&cart_text)),
            ("ENGINE_JS", &engine_js),
        ],
    )?;

    Ok(Bundle {
        title,
        html,
        warnings,
    })
}

fn parse_args(args: &[String]) -> Result<Option<PackArgs>, String> {
    let mut cart = None;
    let mut out = None;
    let mut engine = None;
    let mut template = None;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (arg.as_str(), None),
        };

        let mut value = |name: &str| -> Result<PathBuf, String> {
            if let Some(value) = inline {
                return Ok(PathBuf::from(value));
            }
            index += 1;
            args.get(index)
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} requires a value"))
        };

        match flag {
            "-h" | "--help" => return Ok(None),
            "-o" | "--out" | "--output" => out = Some(value("-o")?),
            "--engine" => engine = Some(value("--engine")?),
            "--template" => template = Some(value("--template")?),
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option `{other}` (try --help)"));
            }
            _ => {
                if cart.is_some() {
                    return Err(format!("unexpected extra argument `{arg}` (try --help)"));
                }
                cart = Some(PathBuf::from(arg));
            }
        }
        index += 1;
    }

    Ok(Some(PackArgs {
        bundle: BundleOptions {
            cart: cart.ok_or("missing <cart> argument (try --help)")?,
            engine,
            template,
        },
        out: out.ok_or("missing -o <out.html> (try --help)")?,
    }))
}

fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

fn read_override_or_embedded(
    path: Option<&Path>,
    embedded: &str,
    embedded_label: &'static str,
) -> Result<(String, String), String> {
    match path {
        Some(path) => Ok((read_text(path)?, path.display().to_string())),
        None => Ok((embedded.to_owned(), embedded_label.to_owned())),
    }
}

fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = haystack.as_bytes().to_ascii_lowercase();
    let needle = needle.as_bytes().to_ascii_lowercase();
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn escape_cart_text(value: &str) -> String {
    value.replace("</", "<\\/")
}

fn render(template: &str, variables: &[(&str, &str)]) -> Result<String, String> {
    let capacity = template.len()
        + variables
            .iter()
            .map(|(_, value)| value.len())
            .sum::<usize>();
    let mut output = String::with_capacity(capacity);
    let mut rest = template;
    let mut seen = Vec::new();

    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or("template has an unterminated `{{` placeholder")?;
        let name = &after[..end];
        let value = variables
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
            .ok_or_else(|| format!("template references unknown placeholder `{{{{{name}}}}}`"))?;
        output.push_str(value);
        if !seen.contains(&name) {
            seen.push(name);
        }
        rest = &after[end + 2..];
    }
    output.push_str(rest);

    for (name, _) in variables {
        if !seen.contains(name) {
            return Err(format!(
                "template is missing the `{{{{{name}}}}}` placeholder"
            ));
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_short_long_and_inline_options() {
        let args = [
            "game.cart",
            "--output=game.html",
            "--engine",
            "engine.js",
            "--template=page.html",
        ]
        .map(str::to_owned);
        let parsed = parse_args(&args).unwrap().unwrap();
        assert_eq!(parsed.bundle.cart, Path::new("game.cart"));
        assert_eq!(parsed.out, Path::new("game.html"));
        assert_eq!(
            parsed.bundle.engine.as_deref(),
            Some(Path::new("engine.js"))
        );
        assert_eq!(
            parsed.bundle.template.as_deref(),
            Some(Path::new("page.html"))
        );
    }

    #[test]
    fn parser_rejects_missing_and_extra_arguments() {
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&["game.cart".into()]).is_err());
        assert!(parse_args(&["a.cart".into(), "b.cart".into(), "-o".into(), "x".into()]).is_err());
        assert!(parse_args(&["--bogus".into()]).is_err());
    }

    #[test]
    fn escapes_html_in_titles() {
        assert_eq!(html_escape("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&#39;");
    }

    #[test]
    fn cart_escaping_round_trips_through_shell_regex() {
        let source = "print('</script>') -- x < y\n";
        let packed = escape_cart_text(source);
        assert!(!packed.contains("</"));
        assert_eq!(packed.replace("<\\/", "</"), source);
    }

    #[test]
    fn renders_without_rescanning_substituted_content() {
        let rendered = render(
            "<t>{{TITLE}}</t>[{{CART_TEXT}}]{{ENGINE_JS}}",
            &[
                ("TITLE", "T"),
                ("CART_TEXT", "{{ENGINE_JS}}"),
                ("ENGINE_JS", "E"),
            ],
        )
        .unwrap();
        assert_eq!(rendered, "<t>T</t>[{{ENGINE_JS}}]E");
    }

    #[test]
    fn render_rejects_unknown_missing_and_unterminated_placeholders() {
        assert!(render("{{NOPE}}", &[("TITLE", "T")]).is_err());
        assert!(render("no placeholders", &[("TITLE", "T")]).is_err());
        assert!(render("{{TITLE", &[("TITLE", "T")]).is_err());
    }

    #[test]
    fn finds_script_terminator_case_insensitively() {
        assert_eq!(find_ignore_ascii_case("ab</SCRIPT>", "</script"), Some(2));
        assert_eq!(find_ignore_ascii_case("all clear", "</script"), None);
    }

    #[test]
    fn embedded_template_has_the_exact_required_placeholders() {
        let rendered = render(
            DEFAULT_TEMPLATE,
            &[("TITLE", "T"), ("CART_TEXT", "C"), ("ENGINE_JS", "E")],
        )
        .unwrap();
        assert!(!rendered.contains("{{"));
        for name in ["TITLE", "CART_TEXT", "ENGINE_JS"] {
            assert_eq!(
                DEFAULT_TEMPLATE.matches(&format!("{{{{{name}}}}}")).count(),
                1
            );
        }
    }

    #[test]
    fn diagnostic_handle_is_installed_before_boot_and_read_only() {
        let install = DEFAULT_TEMPLATE
            .find("window.__console = Object.freeze")
            .expect("diagnostic handle is installed");
        let boot = DEFAULT_TEMPLATE
            .find("await ConsoleEngine()")
            .expect("template boots the engine");
        assert!(install < boot);
        for method in ["status", "screenState", "audioState"] {
            assert!(DEFAULT_TEMPLATE[install..boot].contains(&format!("{method}: function")));
        }
        for unsafe_name in ["reset", "step", "eval", "module", "heap"] {
            assert!(!DEFAULT_TEMPLATE[install..boot].contains(&format!("{unsafe_name}: function")));
        }
    }
}
