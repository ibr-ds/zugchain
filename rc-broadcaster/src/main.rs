#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    rc_broadcaster::main().await
}
