use anyhow::{Context, Result};
use linkify::{LinkFinder, LinkKind};
use readabilityrs::Readability;
use reqwest::header::CONTENT_TYPE;

#[derive(Debug, Clone)]
pub struct ExtractedContent {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
}

pub fn extract_urls(text: &str) -> Vec<String> {
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);

    let mut urls = Vec::new();
    for link in finder.links(text) {
        let url = link.as_str().trim().to_string();
        if (url.starts_with("http://") || url.starts_with("https://")) && !urls.contains(&url) {
            urls.push(url);
        }
    }

    urls
}

pub async fn fetch_html(
    http: &reqwest::Client,
    url: &str,
    max_bytes: usize,
) -> Result<(String, String)> {
    let response = http
        .get(url)
        .header(
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .with_context(|| format!("Failed to fetch {}", url))?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Fetch failed: {} {}", status, url);
    }

    if let Some(content_type) = response.headers().get(CONTENT_TYPE) {
        let content_type = content_type.to_str().unwrap_or_default();
        if !content_type.contains("text/html") && !content_type.contains("application/xhtml") {
            anyhow::bail!("Unsupported content type: {}", content_type);
        }
    }

    if let Some(len) = response.content_length()
        && len as usize > max_bytes
    {
        anyhow::bail!("Content too large ({} bytes)", len);
    }

    let final_url = response.url().to_string();
    let bytes = response
        .bytes()
        .await
        .context("Failed to read response body")?;
    if bytes.len() > max_bytes {
        anyhow::bail!("Content too large ({} bytes)", bytes.len());
    }

    let html = String::from_utf8_lossy(&bytes).to_string();
    Ok((final_url, html))
}

pub fn extract_content(html: &str, base_url: &str) -> Result<ExtractedContent> {
    let readability =
        Readability::new(html, Some(base_url), None).context("Failed to initialize readability")?;
    let article = readability.parse();

    let (title, text) = match article {
        Some(article) => {
            let title = article.title.as_deref().and_then(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            });
            let text = match article.text_content.as_deref() {
                Some(value) if !value.trim().is_empty() => value.to_string(),
                _ => {
                    let fallback = article.content.as_deref().unwrap_or(html);
                    html_to_text_basic(fallback)
                }
            };
            (title, text)
        }
        None => (None, html_to_text_basic(html)),
    };

    if text.trim().is_empty() {
        anyhow::bail!("No readable text extracted");
    }

    Ok(ExtractedContent {
        url: base_url.to_string(),
        title,
        text,
    })
}

fn html_to_text_basic(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_name = String::new();
    let mut end_tag = false;

    for ch in input.chars() {
        if in_tag {
            match ch {
                '/' if tag_name.is_empty() => {
                    end_tag = true;
                }
                '>' => {
                    let tag = tag_name.to_lowercase();
                    if tag == "script" {
                        in_script = !end_tag;
                    } else if tag == "style" {
                        in_style = !end_tag;
                    }
                    in_tag = false;
                    tag_name.clear();
                    end_tag = false;
                    if !in_script && !in_style {
                        out.push(' ');
                    }
                }
                c if c.is_whitespace() => {
                    // Ignore attributes.
                }
                c => {
                    if tag_name.len() < 32 {
                        tag_name.push(c);
                    }
                }
            }
            continue;
        }

        match ch {
            '<' => {
                in_tag = true;
                tag_name.clear();
                end_tag = false;
            }
            _ => {
                if !in_script && !in_style {
                    out.push(ch);
                }
            }
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn extract_content_writes_output_for_fixture() {
        let html = r#"
        <html>
          <head><title>Readability Example</title></head>
          <body>
            <article>
              <h1>Readability Example</h1>
              <p>Hello from readability.</p>
              <p>This is a second paragraph.</p>
            </article>
          </body>
        </html>
        "#;

        let extracted = extract_content(html, "https://example.com").unwrap();
        assert!(extracted.text.contains("Hello from readability"));

        let out_dir = PathBuf::from("target/test-output");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("readability_sample.txt"), &extracted.text).unwrap();
    }
}
