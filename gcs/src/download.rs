use futures_util::StreamExt;
use reqwest::Url;

/// Download smaller files from an HTTP endpoint
pub async fn download_file(url: Url) -> anyhow::Result<Vec<u8>> {
    let response = reqwest::get(url).await?;
    let mut stream = response.bytes_stream();

    let mut bytes = vec![];
    while let Some(Ok(item)) = stream.next().await {
        bytes.extend(item);
    }
    Ok(bytes)
}
