//! `console-pack` — splice a cart + the emscripten engine JS into one
//! self-contained `game.html`.
//!
//! ```text
//! console-pack <cart> -o <out.html> [--engine web/engine.js] [--template web/template.html]
//! ```
//!
//! No dependencies beyond `console-core` (used only to validate the cart and
//! read its title): the argument parsing, escaping and templating are all
//! hand-rolled so the packer stays trivially auditable.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use console_core::Cart;

const USAGE: &str = "\
console-pack — pack a cart + engine into a single self-contained HTML file

USAGE:
    console-pack <cart> -o <out.html> [OPTIONS]

ARGS:
    <cart>                  Path to the .cart file (UTF-8 text)

OPTIONS:
    -o, --out <FILE>        Output HTML file (required)
        --engine <FILE>     Emscripten engine JS
                            [default: web/engine.js]
        --template <FILE>   HTML template with {{TITLE}} / {{CART_TEXT}} /
                            {{ENGINE_JS}} placeholders
                            [default: web/template.html]
    -h, --help              Print this help

DEFAULT PATHS:
    --engine and --template default to paths under the repository root. The
    root is located by looking for a `web/` directory in the current directory,
    then in its ancestors, then in the ancestors of this executable. Running
    from the repository root always works; otherwise pass both flags
    explicitly.

OUTPUT:
    The result has zero external references and runs from file://. The cart
    text is embedded verbatim inside <script type=\"text/cart\"> (with `</`
    escaped as `<\\/`, which the shell undoes at load), so it stays
    human- and agent-editable inside the HTML.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("console-pack: error: {msg}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    cart: PathBuf,
    out: PathBuf,
    engine: Option<PathBuf>,
    template: Option<PathBuf>,
}

/// Hand-rolled argument parsing: one positional, three options, `--k v` and
/// `--k=v` both accepted.
fn parse_args() -> Result<Option<Args>, String> {
    let mut cart: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut engine: Option<PathBuf> = None;
    let mut template: Option<PathBuf> = None;

    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        // Split `--flag=value` into its two halves up front.
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };

        let mut value = |what: &str| -> Result<PathBuf, String> {
            match inline.clone().or_else(|| it.next()) {
                Some(v) => Ok(PathBuf::from(v)),
                None => Err(format!("{what} requires a value")),
            }
        };

        match flag.as_str() {
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
    }

    let cart = cart.ok_or("missing <cart> argument (try --help)")?;
    let out = out.ok_or("missing -o <out.html> (try --help)")?;
    Ok(Some(Args {
        cart,
        out,
        engine,
        template,
    }))
}

fn run() -> Result<(), String> {
    let Some(args) = parse_args()? else {
        print!("{USAGE}");
        return Ok(());
    };

    let engine_path = match args.engine {
        Some(p) => p,
        None => locate_default("web/engine.js")?,
    };
    let template_path = match args.template {
        Some(p) => p,
        None => locate_default("web/template.html")?,
    };

    let cart_text = read_text(&args.cart)?;
    let engine_js = read_text(&engine_path)?;
    let template = read_text(&template_path)?;

    // Validate the cart before writing anything: a broken cart should fail
    // here, not silently produce an HTML file that explodes in the browser.
    let cart = Cart::parse(&cart_text).map_err(|e| format!("{}: {e}", args.cart.display()))?;

    // `</script` anywhere in the engine JS would close the wrapping <script>
    // tag and shred the document. We refuse rather than mangle the JS.
    if let Some(at) = find_ignore_ascii_case(&engine_js, "</script") {
        return Err(format!(
            "{}: engine JS contains `</script` at byte offset {at}; it cannot be \
             embedded in a <script> element. Rebuild the engine (web/build-engine.sh).",
            engine_path.display()
        ));
    }

    // Emscripten's default -sSINGLE_FILE embedding (SINGLE_FILE_BINARY_ENCODE=1)
    // writes the wasm as raw bytes, including NULs, which the HTML parser
    // rewrites to U+FFFD and which stops the packed page from being plain text
    // that a human or agent can safely edit. web/build-engine.sh disables it.
    if !engine_js.is_ascii() {
        eprintln!(
            "console-pack: warning: {} is not pure ASCII (raw wasm embedding?). \
             The packed HTML will depend on exact UTF-8 handling and will not be \
             safely text-editable. Rebuild with -sSINGLE_FILE_BINARY_ENCODE=0.",
            engine_path.display()
        );
    }

    let html = render(
        &template,
        &[
            ("TITLE", &html_escape(cart.title())),
            ("CART_TEXT", &escape_cart_text(&cart_text)),
            ("ENGINE_JS", &engine_js),
        ],
    )?;

    if let Some(dir) = args.out.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        }
    }
    fs::write(&args.out, html.as_bytes())
        .map_err(|e| format!("writing {}: {e}", args.out.display()))?;

    eprintln!(
        "console-pack: {} ({}) -> {} ({} bytes)",
        args.cart.display(),
        cart.title(),
        args.out.display(),
        html.len()
    );
    Ok(())
}

fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// Find `web/engine.js`-style assets without requiring a specific CWD: check
/// the current directory and its ancestors, then the executable's ancestors.
fn locate_default(rel: &str) -> Result<PathBuf, String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        roots.extend(cwd.ancestors().map(Path::to_path_buf));
    }
    if let Ok(exe) = env::current_exe() {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    for root in roots {
        let candidate = root.join(rel);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "could not find `{rel}` in the current directory, any of its ancestors, \
         or next to the executable — run console-pack from the repository root, \
         or pass --engine/--template explicitly"
    ))
}

/// Case-insensitive substring search (ASCII), returning a byte offset.
fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.as_bytes().to_ascii_lowercase();
    let need = needle.as_bytes().to_ascii_lowercase();
    if need.is_empty() || hay.len() < need.len() {
        return None;
    }
    hay.windows(need.len()).position(|w| w == need)
}

/// Escape text destined for HTML *character data* / attribute values.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape cart text for embedding in `<script type="text/cart">`.
///
/// Script elements are raw text: entities are *not* decoded, and the only
/// sequence that can end the element early is `</`. We escape every literal
/// `</` as `<\/`; the shell in `web/template.html` reverses this with
/// `.replace(/<\\\//g, "</")`.
fn escape_cart_text(s: &str) -> String {
    s.replace("</", "<\\/")
}

/// Replace every `{{NAME}}` in `template` with its value, in a single pass so
/// that substituted content is never rescanned. Unknown names are an error.
fn render(template: &str, vars: &[(&str, &str)]) -> Result<String, String> {
    let mut out =
        String::with_capacity(template.len() + vars.iter().map(|v| v.1.len()).sum::<usize>());
    let mut rest = template;
    let mut seen: Vec<&str> = Vec::new();

    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or("template has an unterminated `{{` placeholder")?;
        let name = &after[..end];
        let value = vars
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .ok_or_else(|| format!("template references unknown placeholder `{{{{{name}}}}}`"))?;
        out.push_str(value);
        if !seen.contains(&name) {
            seen.push(name);
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);

    for (name, _) in vars {
        if !seen.contains(name) {
            return Err(format!(
                "template is missing the `{{{{{name}}}}}` placeholder"
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_in_titles() {
        assert_eq!(html_escape("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&#39;");
    }

    #[test]
    fn escapes_only_script_closing_sequences_in_cart_text() {
        assert_eq!(escape_cart_text("a </script> b"), "a <\\/script> b");
        // `<` and `/` on their own must survive untouched (Lua comparisons,
        // division, `--` comments).
        assert_eq!(escape_cart_text("if a < b / c then"), "if a < b / c then");
    }

    #[test]
    fn cart_escaping_round_trips_through_the_shell_regex() {
        let src = "print('</script>') -- x < y\n";
        let packed = escape_cart_text(src);
        assert!(!packed.contains("</"));
        assert_eq!(packed.replace("<\\/", "</"), src);
    }

    #[test]
    fn renders_each_placeholder_once_without_rescanning() {
        let out = render(
            "<t>{{TITLE}}</t>[{{CART_TEXT}}]{{ENGINE_JS}}",
            &[
                ("TITLE", "T"),
                ("CART_TEXT", "{{ENGINE_JS}}"),
                ("ENGINE_JS", "E"),
            ],
        )
        .unwrap();
        assert_eq!(out, "<t>T</t>[{{ENGINE_JS}}]E");
    }

    #[test]
    fn render_rejects_unknown_and_missing_placeholders() {
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
    fn real_template_has_exactly_the_placeholders_we_supply() {
        // Guards against the template regressing to a state where a stray
        // `{{ENGINE_JS}}` (e.g. inside a comment) gets a second expansion.
        let Ok(path) = super::locate_default("web/template.html") else {
            return; // not running from the repo — nothing to check
        };
        let template = fs::read_to_string(path).unwrap();
        let rendered = render(
            &template,
            &[("TITLE", "T"), ("CART_TEXT", "C"), ("ENGINE_JS", "E")],
        )
        .unwrap();
        assert!(!rendered.contains("{{"));
        // Each placeholder should appear exactly once in the template.
        assert_eq!(template.matches("{{ENGINE_JS}}").count(), 1);
        assert_eq!(template.matches("{{CART_TEXT}}").count(), 1);
        assert_eq!(template.matches("{{TITLE}}").count(), 1);
    }

    #[test]
    fn diagnostic_handle_is_immutable_installed_before_boot_and_read_only() {
        let Ok(path) = super::locate_default("web/template.html") else {
            return;
        };
        let template = fs::read_to_string(path).unwrap();
        let install = template
            .find("window.__console = Object.freeze")
            .expect("diagnostic handle is installed");
        let boot = template
            .find("await ConsoleEngine()")
            .expect("template boots the engine");
        assert!(
            install < boot,
            "diagnostics must exist while the engine boots"
        );
        for method in ["status", "screenState", "audioState"] {
            assert!(
                template[install..boot].contains(&format!("{method}: function")),
                "missing read-only diagnostic method {method}"
            );
        }
        for unsafe_name in ["reset", "step", "eval", "module", "heap"] {
            assert!(
                !template[install..boot].contains(&format!("{unsafe_name}: function")),
                "diagnostic handle must not expose {unsafe_name}"
            );
        }
    }
}
