use anyhow::{anyhow, ensure};
use std::time::Duration;

/// Try to convert float seconds into a `Duration`.
///
/// TODO replace with <https://github.com/rust-lang/rust/issues/83400>.
pub fn try_from_secs_f64(secs: f64) -> anyhow::Result<Duration> {
    ensure!(
        secs.is_sign_positive(),
        "invalid negative seconds: {secs:?}"
    );
    ensure!(secs.is_finite(), "invalid infinite seconds: {secs:?}");
    ensure!(secs < i64::MAX as f64, "invalid length seconds: {secs:?}");
    let r = std::panic::catch_unwind(|| Duration::from_secs_f64(secs));
    r.map_err(|err| match err.downcast::<String>() {
        Ok(e) => anyhow!("{e}: {secs:?}"),
        _ => anyhow!("Duration::from_secs_f64 panicked: {secs:?}"),
    })
}
