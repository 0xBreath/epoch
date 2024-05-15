use std::path::PathBuf;

#[derive(serde::Deserialize, Clone, Debug)]
pub struct DatabaseSettings {
    pub username: Option<String>,
    pub password: Option<String>,
    pub port: Option<u16>,
    pub host: Option<String>,
    pub connection_string: Option<String>,
    pub database_name: Option<String>,
    pub threads: Option<usize>,
    pub batch_size: Option<usize>,
    pub panic_on_db_error: Option<bool>,
}

impl DatabaseSettings {
    pub fn new_with_config_path(
        config_path: PathBuf,
    ) -> anyhow::Result<DatabaseSettings, config::ConfigError> {
        let settings = config::Config::builder()
            .add_source(config::File::from(config_path))
            .build()?;

        settings.try_deserialize::<DatabaseSettings>()
    }

    pub fn new_from_url(
        connection_url: String,
    ) -> anyhow::Result<DatabaseSettings, config::ConfigError> {
        let settings = config::Config::builder()
            .set_default("connection_string", connection_url)?
            .build()?;
        settings.try_deserialize::<DatabaseSettings>()
    }
}
