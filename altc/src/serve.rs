use axum::Router;
use std::net::SocketAddr;
use std::path::Path;

/// Which listeners to open. Both can run at once; either can be turned off,
/// but not both — an agent nobody can reach is a silent failure.
#[derive(Debug, Clone, PartialEq)]
pub struct Listeners {
    pub http: Option<u16>,
    pub https: Option<u16>,
}

/// `0` disables a listener, so a deployment can turn one off without the
/// config needing a separate boolean that could disagree with the port.
fn port_from_env(var: &str, default: u16) -> Option<u16> {
    match std::env::var(var) {
        Err(_) => Some(default),
        Ok(raw) => match raw.trim().parse::<u16>() {
            Ok(0) => None,
            Ok(p) => Some(p),
            Err(_) => {
                tracing::error!("{var}={raw} is not a port number; using {default}");
                Some(default)
            }
        },
    }
}

impl Listeners {
    /// HTTPS stays on 47990 exactly as before so existing clients, paired
    /// devices and tokens keep working. HTTP is additive and off unless asked
    /// for; a packaged install (P8) turns it on so a new user meets a page
    /// rather than a certificate warning.
    pub fn from_env() -> Self {
        let https = port_from_env("ALTC_API_PORT", 47990);
        let http = match std::env::var("ALTC_HTTP_PORT") {
            Ok(raw) => match raw.trim().parse::<u16>() {
                Ok(0) => None,
                Ok(p) => Some(p),
                Err(_) => {
                    tracing::error!("ALTC_HTTP_PORT={raw} is not a port number; HTTP disabled");
                    None
                }
            },
            Err(_) => None,
        };
        Self { http, https }
    }

    pub fn describe(&self) -> String {
        match (self.http, self.https) {
            (Some(h), Some(s)) => format!("http://0.0.0.0:{h} and https://0.0.0.0:{s}"),
            (Some(h), None) => format!("http://0.0.0.0:{h}"),
            (None, Some(s)) => format!("https://0.0.0.0:{s}"),
            (None, None) => "nothing".to_string(),
        }
    }
}

/// Says plainly what is exposed. HTTP carries the bearer token in clear, which
/// is fine inside a VPN and fine on a home LAN, and a disaster if someone
/// port-forwards it — so it is stated at every startup rather than left to be
/// discovered.
pub fn warn_about_http(listeners: &Listeners) {
    if let Some(port) = listeners.http {
        tracing::warn!(
            "HTTP is enabled on port {port}: bearer tokens cross the network in clear. \
             Safe on a LAN or inside a VPN such as Tailscale. Do NOT port-forward it."
        );
    }
}

/// Serves the same router on whichever listeners are configured.
pub async fn run(app: Router, listeners: Listeners, cert: &Path, key: &Path) {
    let mut tasks = Vec::new();

    if let Some(port) = listeners.http {
        let app = app.clone();
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        tasks.push(tokio::spawn(async move {
            axum_server::bind(addr)
                .serve(app.into_make_service())
                .await
                .expect("http server");
        }));
    }

    if let Some(port) = listeners.https {
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
            .await
            .expect("load tls cert");
        let app = app.clone();
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        tasks.push(tokio::spawn(async move {
            axum_server::bind_rustls(addr, tls)
                .serve(app.into_make_service())
                .await
                .expect("https server");
        }));
    }

    for task in tasks {
        let _ = task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env vars are process-global, so these run under one lock rather than
    /// racing each other for the same variables.
    fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        let out = f();
        for (k, v) in saved {
            match v {
                Some(v) => unsafe { std::env::set_var(&k, v) },
                None => unsafe { std::env::remove_var(&k) },
            }
        }
        out
    }

    #[test]
    fn defaults_keep_todays_behaviour_exactly() {
        // Existing clients, paired devices and tokens all assume 47990 over
        // TLS. Adding HTTP must not move that.
        let l = with_env(
            &[("ALTC_API_PORT", None), ("ALTC_HTTP_PORT", None)],
            Listeners::from_env,
        );
        assert_eq!(l.https, Some(47990));
        assert_eq!(l.http, None, "HTTP must be opt-in, not a silent downgrade");
    }

    #[test]
    fn http_is_enabled_by_setting_a_port() {
        let l = with_env(
            &[("ALTC_API_PORT", None), ("ALTC_HTTP_PORT", Some("47989"))],
            Listeners::from_env,
        );
        assert_eq!(l.http, Some(47989));
        assert_eq!(l.https, Some(47990), "enabling HTTP must not disable HTTPS");
    }

    #[test]
    fn zero_disables_a_listener() {
        let l = with_env(
            &[
                ("ALTC_API_PORT", Some("0")),
                ("ALTC_HTTP_PORT", Some("8080")),
            ],
            Listeners::from_env,
        );
        assert_eq!(l.https, None);
        assert_eq!(l.http, Some(8080));
    }

    #[test]
    fn a_nonsense_port_falls_back_rather_than_crashing() {
        // A typo in a compose file must not leave the agent unreachable.
        let l = with_env(
            &[
                ("ALTC_API_PORT", Some("not-a-port")),
                ("ALTC_HTTP_PORT", None),
            ],
            Listeners::from_env,
        );
        assert_eq!(l.https, Some(47990));
    }

    #[test]
    fn describe_says_what_is_actually_exposed() {
        let both = Listeners {
            http: Some(80),
            https: Some(443),
        };
        assert!(both.describe().contains("http://0.0.0.0:80"));
        assert!(both.describe().contains("https://0.0.0.0:443"));
        let only_https = Listeners {
            http: None,
            https: Some(47990),
        };
        assert!(!only_https.describe().contains("http://"));
    }
}
