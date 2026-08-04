//! Bundle a cart and serve the playable page over a tiny local HTTP server.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use crate::pack::{self, Bundle, BundleOptions};

pub const USAGE: &str = r#"console serve — bundle and locally serve a cart or project

USAGE:
    console serve <cart|project> [OPTIONS]

ARGS:
    <cart|project>          Path to a .cart, console.toml, or project directory

OPTIONS:
        --host <HOST>       Interface to bind [default: 127.0.0.1]
        --port <PORT>       TCP port; use 0 to choose a free port [default: 8000]
        --engine <FILE>     Override the embedded browser engine
        --template <FILE>   Override the embedded HTML template
        --once              Serve one HTTP connection, then exit
    -h, --help              Print this help

The cart or project is compiled and bundled again on every page refresh, so
saved source edits appear without restarting the server. Only / and
/index.html are served. HTTP Host
must match --host; wildcard binds accept IP-literal Host values only.
"#;

#[derive(Debug, Clone)]
struct ServeArgs {
    bundle: BundleOptions,
    host: String,
    port: u16,
    once: bool,
}

pub fn cli_serve(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return 0;
        }
        Err(error) => {
            eprintln!("console serve: error: {error}");
            return 2;
        }
    };

    let initial = match pack::bundle(&args.bundle) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("console serve: error: {error}");
            return 1;
        }
    };
    print_warnings(&initial);
    let title = initial.title.clone();
    drop(initial);

    let listener = match TcpListener::bind((args.host.as_str(), args.port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!(
                "console serve: error: binding {}:{}: {error}",
                args.host, args.port
            );
            return 1;
        }
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => {
            eprintln!("console serve: error: reading listener address: {error}");
            return 1;
        }
    };

    println!("{}", browser_url(&args.host, address));
    if let Err(error) = std::io::stdout().flush() {
        eprintln!("console serve: error: writing URL: {error}");
        return 1;
    }
    eprintln!(
        "console serve: serving {} ({}) — press Ctrl-C to stop",
        args.bundle.cart.display(),
        title
    );

    match run_server(listener, &args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("console serve: error: {error}");
            1
        }
    }
}

fn parse_args(args: &[String]) -> Result<Option<ServeArgs>, String> {
    let mut cart = None;
    let mut host = "127.0.0.1".to_owned();
    let mut port = 8000;
    let mut engine = None;
    let mut template = None;
    let mut once = false;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (arg.clone(), None),
        };

        let value = |index: &mut usize, name: &str| -> Result<String, String> {
            if let Some(value) = &inline {
                return Ok(value.clone());
            }
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };

        match flag.as_str() {
            "-h" | "--help" => return Ok(None),
            "--host" => host = value(&mut index, "--host")?,
            "--port" => {
                let raw = value(&mut index, "--port")?;
                port = raw.parse().map_err(|_| {
                    format!("--port must be an integer from 0 to 65535, got `{raw}`")
                })?;
            }
            "--engine" => engine = Some(PathBuf::from(value(&mut index, "--engine")?)),
            "--template" => {
                template = Some(PathBuf::from(value(&mut index, "--template")?));
            }
            "--once" => {
                if inline.is_some() {
                    return Err("--once does not take a value".to_owned());
                }
                once = true;
            }
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

    if host.is_empty() {
        return Err("--host cannot be empty".to_owned());
    }
    Ok(Some(ServeArgs {
        bundle: BundleOptions {
            cart: cart.ok_or("missing <cart|project> argument (try --help)")?,
            engine,
            template,
        },
        host,
        port,
        once,
    }))
}

fn run_server(listener: TcpListener, args: &ServeArgs) -> Result<(), String> {
    let port = listener
        .local_addr()
        .map_err(|error| format!("reading listener address: {error}"))?
        .port();
    for incoming in listener.incoming() {
        let mut stream = incoming.map_err(|error| format!("accepting connection: {error}"))?;
        if let Err(error) = handle_connection(&mut stream, args, port) {
            eprintln!("console serve: request error: {error}");
            let _ = write_response(
                &mut stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                b"bad request\n",
                false,
            );
        }
        if args.once {
            break;
        }
    }
    Ok(())
}

fn handle_connection(stream: &mut TcpStream, args: &ServeArgs, port: u16) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("setting read timeout: {error}"))?;
    let request = read_request(stream)?;
    let head = request.method == "HEAD";
    if request.method != "GET" && !head {
        return write_response(
            stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"method not allowed\n",
            false,
        );
    }

    if !host_allowed(&request.host, &args.host, port) {
        return write_response(
            stream,
            "421 Misdirected Request",
            "text/plain; charset=utf-8",
            b"host not allowed\n",
            head,
        );
    }

    let path = request.target.split('?').next().unwrap_or(&request.target);
    if !matches!(path, "/" | "/index.html") {
        return write_response(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found\n",
            head,
        );
    }

    let bundle = match pack::bundle(&args.bundle) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("console serve: bundle failed: {error}");
            return write_response(
                stream,
                "500 Internal Server Error",
                "text/plain; charset=utf-8",
                format!("cart bundle failed: {error}\n").as_bytes(),
                head,
            );
        }
    };
    print_warnings(&bundle);
    write_response(
        stream,
        "200 OK",
        "text/html; charset=utf-8",
        bundle.html.as_bytes(),
        head,
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: String,
    target: String,
    host: String,
}

fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    const LIMIT: usize = 16 * 1024;
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let count = stream
            .read(&mut chunk)
            .map_err(|error| format!("reading request: {error}"))?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() >= LIMIT {
            return Err(format!("request headers exceed {LIMIT} bytes"));
        }
    }
    if request.len() >= LIMIT {
        return Err(format!("request headers exceed {LIMIT} bytes"));
    }
    let request = std::str::from_utf8(&request)
        .map_err(|_| "request headers are not valid UTF-8".to_owned())?;
    let mut lines = request.lines();
    let request_line = lines.next().ok_or_else(|| "empty request".to_owned())?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "request has no method".to_owned())?;
    let target = parts
        .next()
        .ok_or_else(|| "request has no target".to_owned())?;
    let version = parts
        .next()
        .ok_or_else(|| "request has no HTTP version".to_owned())?;
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return Err("malformed request line".to_owned());
    }

    let mut host = None;
    for line in lines {
        if line.is_empty() || line == "\r" {
            break;
        }
        let (name, value) = line
            .trim_end_matches('\r')
            .split_once(':')
            .ok_or_else(|| "malformed request header".to_owned())?;
        if name.eq_ignore_ascii_case("host") {
            if host.is_some() {
                return Err("request has more than one Host header".to_owned());
            }
            let value = value.trim();
            if value.is_empty() {
                return Err("request has an empty Host header".to_owned());
            }
            host = Some(value.to_owned());
        }
    }

    Ok(Request {
        method: method.to_owned(),
        target: target.to_owned(),
        host: host.ok_or_else(|| "request has no Host header".to_owned())?,
    })
}

/// Validate the HTTP authority before returning source-bearing HTML. This is
/// especially important on loopback: without it, a hostile public hostname
/// can DNS-rebind to 127.0.0.1 and read the locally served cart.
fn host_allowed(authority: &str, configured_host: &str, port: u16) -> bool {
    let Some((host, request_port)) = split_authority(authority) else {
        return false;
    };
    if request_port.unwrap_or(80) != port {
        return false;
    }

    match configured_host {
        "0.0.0.0" | "::" | "[::]" => host.parse::<IpAddr>().is_ok(),
        configured => match (
            host.parse::<IpAddr>(),
            configured.trim_matches(['[', ']']).parse::<IpAddr>(),
        ) {
            (Ok(actual), Ok(expected)) => actual == expected,
            _ => host.eq_ignore_ascii_case(configured),
        },
    }
}

fn split_authority(authority: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let suffix = &rest[end + 1..];
        let port = match suffix.strip_prefix(':') {
            Some(value) => Some(value.parse().ok()?),
            None if suffix.is_empty() => None,
            None => return None,
        };
        return Some((host, port));
    }
    if authority.matches(':').count() > 1 {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Some((host, Some(port.parse().ok()?))),
        Some(_) => None,
        None if !authority.is_empty() => Some((authority, None)),
        None => None,
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    head: bool,
) -> Result<(), String> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .map_err(|error| format!("writing response headers: {error}"))?;
    if !head {
        stream
            .write_all(body)
            .map_err(|error| format!("writing response body: {error}"))?;
    }
    stream
        .flush()
        .map_err(|error| format!("flushing response: {error}"))
}

fn browser_url(host: &str, address: SocketAddr) -> String {
    let display_host = match host {
        "0.0.0.0" => "127.0.0.1".to_owned(),
        "::" | "[::]" => "[::1]".to_owned(),
        host if host.contains(':') && !host.starts_with('[') => format!("[{host}]"),
        host => host.to_owned(),
    };
    format!("http://{display_host}:{}/", address.port())
}

fn print_warnings(bundle: &Bundle) {
    for warning in &bundle.warnings {
        eprintln!("console serve: warning: {warning}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_inline_overrides() {
        let parsed = parse_args(&["game.cart".into()]).unwrap().unwrap();
        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 8000);
        assert!(!parsed.once);

        let parsed = parse_args(&[
            "game.cart".into(),
            "--host=0.0.0.0".into(),
            "--port".into(),
            "0".into(),
            "--once".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(parsed.host, "0.0.0.0");
        assert_eq!(parsed.port, 0);
        assert!(parsed.once);
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&["game.cart".into(), "--port=nope".into()]).is_err());
        assert!(parse_args(&["game.cart".into(), "--once=yes".into()]).is_err());
        assert!(parse_args(&["game.cart".into(), "--bogus".into()]).is_err());
    }

    #[test]
    fn browser_urls_handle_wildcard_and_ipv6_hosts() {
        let v4: SocketAddr = "0.0.0.0:8123".parse().unwrap();
        let v6: SocketAddr = "[::]:8123".parse().unwrap();
        assert_eq!(browser_url("0.0.0.0", v4), "http://127.0.0.1:8123/");
        assert_eq!(browser_url("::", v6), "http://[::1]:8123/");
        assert_eq!(browser_url("::1", v6), "http://[::1]:8123/");
    }

    #[test]
    fn host_validation_rejects_dns_rebinding_and_wrong_ports() {
        assert!(host_allowed("127.0.0.1:8000", "127.0.0.1", 8000));
        assert!(!host_allowed("attacker.example:8000", "127.0.0.1", 8000));
        assert!(!host_allowed("127.0.0.1:9000", "127.0.0.1", 8000));
        assert!(host_allowed("[::1]:8000", "::1", 8000));
        assert!(!host_allowed("[::2]:8000", "::1", 8000));
        assert!(host_allowed("192.0.2.4:8000", "0.0.0.0", 8000));
        assert!(!host_allowed("attacker.example:8000", "0.0.0.0", 8000));
        assert!(host_allowed("localhost", "localhost", 80));
    }
}
