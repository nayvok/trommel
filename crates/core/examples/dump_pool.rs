//! Debug helper: print the parsed pool statistics for the configured wallet.
fn main() -> anyhow::Result<()> {
    let cfg = pearl_miner_core::config::Config::load_or_default();
    match pearl_miner_core::pool::query(&cfg) {
        Ok(stats) => println!("{}", serde_json::to_string_pretty(&stats)?),
        Err(err) => println!("ошибка: {err}"),
    }
    Ok(())
}
