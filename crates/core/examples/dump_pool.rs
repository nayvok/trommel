//! Debug helper: print the parsed pool statistics for the configured wallet.
fn main() -> anyhow::Result<()> {
    let cfg = trommel_core::config::Config::load_or_default();
    match trommel_core::pool::query(&cfg) {
        Ok(stats) => println!("{}", serde_json::to_string_pretty(&stats)?),
        Err(err) => println!("ошибка: {err}"),
    }
    Ok(())
}
