use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("не удалось открыть {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ошибка JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ошибка ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("некорректный архив XD: {0}")]
    InvalidXd(String),
}

pub type Result<T> = std::result::Result<T, AppError>;
