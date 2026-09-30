#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = fh::ui::cli::main(args).await;
    std::process::exit(code);
}
