//! Gemini Embedding 2 integration and local similarity helpers.
//!
//! Embeddings 2 does not accept the `task_type` request field.  Retrieval
//! instructions are included in the text prompt as recommended by Google's
//! API documentation, so callers only need to indicate whether the input is
//! a search query or an indexed document.

use serde::Deserialize;

const EMBED_ENDPOINT: &str =
    "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2:embedContent";
const OUTPUT_DIMENSIONALITY: u16 = 768;

#[derive(Deserialize)]
struct EmbedResponse {
    #[serde(default)]
    embedding: Option<EmbeddingValues>,
    // The SDK calls this field `embeddings`; accepting it also keeps this
    // client tolerant of API gateways that return the batch-shaped response.
    #[serde(default)]
    embeddings: Vec<EmbeddingValues>,
}

#[derive(Deserialize)]
struct EmbeddingValues {
    values: Vec<f32>,
}

/// Generate an embedding for a retrieval query or an indexed document.
///
/// `is_query` selects the asymmetric retrieval prompt recommended for
/// `gemini-embedding-2`: queries use a search task prefix, while documents
/// use the title/text structure. The API key is read from `GEMINI_API_KEY`.
pub async fn embed(text: &str, is_query: bool) -> Result<Vec<f32>, String> {
    let key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| "GEMINI_API_KEY is not configured".to_string())?;
    if key.trim().is_empty() {
        return Err("GEMINI_API_KEY is not configured".to_string());
    }

    let formatted = if is_query {
        format!("task: search result | query: {text}")
    } else {
        format!("title: none | text: {text}")
    };
    let body = serde_json::json!({
        "model": "models/gemini-embedding-2",
        "content": { "parts": [{ "text": formatted }] },
        "output_dimensionality": OUTPUT_DIMENSIONALITY,
    });

    let response = reqwest::Client::new()
        .post(EMBED_ENDPOINT)
        .header("x-goog-api-key", key)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("Gemini embedding request failed: {error}"))?;
    let status = response.status();
    let payload = response
        .text()
        .await
        .map_err(|error| format!("failed reading Gemini embedding response: {error}"))?;
    if !status.is_success() {
        return Err(format!("Gemini embedding API returned {status}: {payload}"));
    }

    let parsed: EmbedResponse = serde_json::from_str(&payload)
        .map_err(|error| format!("invalid Gemini embedding response: {error}"))?;
    parsed
        .embedding
        .or_else(|| parsed.embeddings.into_iter().next())
        .map(|embedding| embedding.values)
        .filter(|values| !values.is_empty())
        .ok_or_else(|| "Gemini embedding response contained no values".to_string())
}

/// Return cosine similarity in the range [-1, 1]. Mismatched or zero-length
/// vectors have no meaningful similarity and return 0.
pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let (dot, left_norm, right_norm) = left.iter().zip(right).fold(
        (0.0_f32, 0.0_f32, 0.0_f32),
        |(dot, left_norm, right_norm), (&a, &b)| {
            (dot + a * b, left_norm + a * a, right_norm + b * b)
        },
    );
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if denominator == 0.0 {
        0.0
    } else {
        (dot / denominator).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::cosine;

    #[test]
    fn cosine_scores_direction_and_handles_invalid_vectors() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 1.0]) - 0.70710677).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }
}
