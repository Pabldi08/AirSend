use tracing_subscriber::EnvFilter;

fn resolve_log_dir() -> Option<std::path::PathBuf> {
    const APP_DIRNAME: &str = "ConexionAirPlay";
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA")?;
        Some(
            std::path::PathBuf::from(base)
                .join(APP_DIRNAME)
                .join("logs"),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            std::path::PathBuf::from(home)
                .join("Library")
                .join("Logs")
                .join(APP_DIRNAME),
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
            return Some(std::path::PathBuf::from(xdg).join(APP_DIRNAME).join("logs"));
        }
        let home = std::env::var_os("HOME")?;
        Some(
            std::path::PathBuf::from(home)
                .join(".local")
                .join("state")
                .join(APP_DIRNAME)
                .join("logs"),
        )
    }
}

/// Configura `tracing`: stdout siempre + archivo rotado por día si el directorio
/// de logs es resoluble y escribible. Devuelve el `WorkerGuard` que mantiene
/// vivo el writer non-blocking (se debe retener todo el lifetime del programa;
/// si se dropea, los logs pendientes se pierden).
pub fn init_tracing() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_ansi(true);

    let (file_layer, guard) = match resolve_log_dir() {
        Some(dir) => match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                let appender = tracing_appender::rolling::daily(&dir, "app.log");
                let (writer, guard) = tracing_appender::non_blocking(appender);
                let layer = tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .with_target(true);
                eprintln!("→ logs a {}", dir.display());
                (Some(layer), Some(guard))
            }
            Err(e) => {
                eprintln!("→ no pude crear dir de logs {}: {e}", dir.display());
                (None, None)
            }
        },
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer.map(|l| l.boxed()))
        .try_init()
        .ok()?;

    guard
}
