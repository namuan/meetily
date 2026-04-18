// Model manager for built-in AI models backed by local files.
// It links compatible models from the local Hugging Face cache into the app models directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;

use super::models::{
    get_available_models, get_existing_model_path, get_huggingface_cached_model_path, get_model_by_name,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub downloaded_mb: f64,
    pub total_mb: f64,
    pub speed_mbps: f64,
    pub percent: u8,
}

impl DownloadProgress {
    pub fn new(downloaded: u64, total: u64, speed_mbps: f64) -> Self {
        let percent = if total > 0 {
            ((downloaded as f64 / total as f64) * 100.0) as u8
        } else {
            0
        };

        Self {
            downloaded_bytes: downloaded,
            total_bytes: total,
            downloaded_mb: downloaded as f64 / (1024.0 * 1024.0),
            total_mb: total as f64 / (1024.0 * 1024.0),
            speed_mbps,
            percent,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStatus {
    NotDownloaded,
    Downloading { progress: u8 },
    Available,
    Corrupted { file_size: u64, expected_min_size: u64 },
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub display_name: String,
    pub status: ModelStatus,
    pub path: PathBuf,
    pub size_mb: u64,
    pub context_size: u32,
    pub description: String,
    pub gguf_file: String,
}

pub struct ModelManager {
    models_dir: PathBuf,
    available_models: Arc<RwLock<HashMap<String, ModelInfo>>>,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        Self::new_with_models_dir(None)
    }

    pub fn new_with_models_dir(models_dir: Option<PathBuf>) -> Result<Self> {
        let models_dir = if let Some(dir) = models_dir {
            dir
        } else {
            let current_dir = std::env::current_dir()
                .map_err(|e| anyhow!("Failed to get current directory: {}", e))?;

            if cfg!(debug_assertions) {
                current_dir.join("models").join("summary")
            } else {
                log::warn!("ModelManager: No models directory provided, using fallback path");
                dirs::data_dir()
                    .or_else(dirs::home_dir)
                    .ok_or_else(|| anyhow!("Could not find system data directory"))?
                    .join("Meetily")
                    .join("models")
                    .join("summary")
            }
        };

        log::info!(
            "Built-in AI ModelManager using directory: {}",
            models_dir.display()
        );

        Ok(Self {
            models_dir,
            available_models: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn init(&self) -> Result<()> {
        if !self.models_dir.exists() {
            fs::create_dir_all(&self.models_dir).await?;
            log::info!("Created models directory: {}", self.models_dir.display());
        }

        self.scan_models().await
    }

    pub async fn scan_models(&self) -> Result<()> {
        let model_defs = get_available_models();
        let mut models_map = HashMap::new();

        for model_def in model_defs {
            let model_path = self.models_dir.join(&model_def.gguf_file);
            let resolved_path = get_existing_model_path(&self.app_data_dir(), &model_def);

            let status = if let Some(existing_path) = resolved_path.as_ref() {
                match fs::metadata(existing_path).await {
                    Ok(metadata) => self.validate_model_size(&model_def.name, model_def.size_mb, metadata.len()),
                    Err(e) => ModelStatus::Error(format!("Failed to read metadata: {}", e)),
                }
            } else {
                ModelStatus::NotDownloaded
            };

            let model_info = ModelInfo {
                name: model_def.name.clone(),
                display_name: model_def.display_name.clone(),
                status,
                path: resolved_path.unwrap_or(model_path),
                size_mb: model_def.size_mb,
                context_size: model_def.context_size,
                description: model_def.description.clone(),
                gguf_file: model_def.gguf_file.clone(),
            };

            models_map.insert(model_def.name.clone(), model_info);
        }

        let mut models = self.available_models.write().await;
        *models = models_map;
        Ok(())
    }

    pub async fn list_models(&self) -> Vec<ModelInfo> {
        self.available_models
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    pub async fn get_model_info(&self, model_name: &str) -> Option<ModelInfo> {
        self.available_models.read().await.get(model_name).cloned()
    }

    pub async fn is_model_ready(&self, model_name: &str, refresh: bool) -> bool {
        if refresh && self.scan_models().await.is_err() {
            return false;
        }

        matches!(
            self.get_model_info(model_name).await.map(|info| info.status),
            Some(ModelStatus::Available)
        )
    }

    pub async fn download_model(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(u8) + Send>>,
    ) -> Result<()> {
        let detailed_callback = progress_callback.map(|cb| {
            Box::new(move |p: DownloadProgress| cb(p.percent)) as Box<dyn Fn(DownloadProgress) + Send>
        });
        self.download_model_detailed(model_name, detailed_callback).await
    }

    pub async fn download_model_detailed(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(DownloadProgress) + Send>>,
    ) -> Result<()> {
        let model_def = get_model_by_name(model_name)
            .ok_or_else(|| anyhow!("Unknown model: {}", model_name))?;
        let target_path = self.models_dir.join(&model_def.gguf_file);

        if target_path.exists() {
            self.validate_gguf_file(&target_path).await?;
            if let Some(ref callback) = progress_callback {
                let total = fs::metadata(&target_path).await?.len();
                callback(DownloadProgress::new(total, total, 0.0));
            }
            self.mark_model_available(model_name, target_path).await?;
            return Ok(());
        }

        let cached_path = get_huggingface_cached_model_path(&model_def).ok_or_else(|| {
            anyhow!(
                "Model '{}' not found locally. Expected Hugging Face cache entry for repo {:?} and file '{}'.",
                model_name,
                model_def.huggingface_repo,
                model_def.gguf_file
            )
        })?;

        self.validate_gguf_file(&cached_path).await?;
        self.link_cached_model(&cached_path, &target_path).await?;

        if let Some(ref callback) = progress_callback {
            let total = fs::metadata(&target_path).await?.len();
            callback(DownloadProgress::new(total, total, 0.0));
        }

        self.mark_model_available(model_name, target_path).await
    }

    pub async fn cancel_download(&self, _model_name: &str) -> Result<()> {
        Ok(())
    }

    pub async fn delete_model(&self, model_name: &str) -> Result<()> {
        let model_def = get_model_by_name(model_name)
            .ok_or_else(|| anyhow!("Unknown model: {}", model_name))?;

        let file_path = self.models_dir.join(&model_def.gguf_file);
        if file_path.exists() {
            fs::remove_file(&file_path).await?;
        }

        let mut models = self.available_models.write().await;
        if let Some(model_info) = models.get_mut(model_name) {
            model_info.status = ModelStatus::NotDownloaded;
            model_info.path = file_path;
        }

        Ok(())
    }

    pub fn get_models_directory(&self) -> PathBuf {
        self.models_dir.clone()
    }

    async fn mark_model_available(&self, model_name: &str, path: PathBuf) -> Result<()> {
        let mut models = self.available_models.write().await;
        if let Some(model_info) = models.get_mut(model_name) {
            model_info.status = ModelStatus::Available;
            model_info.path = path;
        }
        Ok(())
    }

    async fn link_cached_model(&self, source_path: &Path, target_path: &Path) -> Result<()> {
        if !self.models_dir.exists() {
            fs::create_dir_all(&self.models_dir).await?;
        }

        if target_path.exists() {
            fs::remove_file(target_path).await?;
        }

        let source_path = source_path.to_path_buf();
        let target_path = target_path.to_path_buf();
        tokio::task::spawn_blocking(move || create_symlink(&source_path, &target_path))
            .await
            .map_err(|e| anyhow!("Failed to join symlink task: {}", e))??;

        Ok(())
    }

    async fn validate_gguf_file(&self, path: &PathBuf) -> Result<()> {
        let mut file = fs::File::open(path).await?;

        use tokio::io::AsyncReadExt;
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic).await?;

        if &magic == b"GGUF" || &magic == b"ggjt" || &magic == b"ggla" || &magic == b"ggml" {
            Ok(())
        } else {
            Err(anyhow!(
                "Invalid model file: magic number {:?} doesn't match GGUF/GGML",
                magic
            ))
        }
    }

    fn validate_model_size(&self, model_name: &str, expected_size_mb: u64, file_size_bytes: u64) -> ModelStatus {
        let file_size_mb = file_size_bytes / (1024 * 1024);
        let expected_min = (expected_size_mb as f64 * 0.9) as u64;
        let expected_max = (expected_size_mb as f64 * 1.1) as u64;

        if file_size_mb >= expected_min && file_size_mb <= expected_max {
            log::info!("Model '{}': AVAILABLE", model_name);
            ModelStatus::Available
        } else {
            log::warn!(
                "Model '{}': CORRUPTED (size mismatch: {} MB, expected {} MB)",
                model_name,
                file_size_mb,
                expected_size_mb
            );
            ModelStatus::Corrupted {
                file_size: file_size_mb,
                expected_min_size: expected_min,
            }
        }
    }

    fn app_data_dir(&self) -> PathBuf {
        self.models_dir
            .parent()
            .and_then(|path| path.parent())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }
}

#[cfg(target_family = "unix")]
fn create_symlink(source_path: &Path, target_path: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source_path, target_path)
        .map_err(|e| anyhow!("Failed to create symlink: {}", e))
}

#[cfg(target_family = "windows")]
fn create_symlink(source_path: &Path, target_path: &Path) -> Result<()> {
    std::os::windows::fs::symlink_file(source_path, target_path)
        .map_err(|e| anyhow!("Failed to create symlink: {}", e))
}
