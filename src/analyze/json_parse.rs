use std::fs::File;
use std::io::Read;
pub struct JsonParser{
    file: String,
}

impl JsonParser {
    pub fn new(file: String) -> Self {
        Self { file }
    }

    pub fn parse(&self) -> Result<String, std::io::Error> {
        let mut file = File::open(&self.file)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        Ok(contents)
    }

    pub fn set_seed(&self, seed: &u32) -> Result<(), Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(&self.file).unwrap_or_default();
        let mut json: serde_json::Value = if contents.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&contents)?
        };
        if !json.is_object() {
            json = serde_json::json!({});
        }
        json["seed"] = serde_json::json!(seed);
        std::fs::write(&self.file, serde_json::to_string_pretty(&json)?)?;
        Ok(())
    }
    pub fn get_seed(&self) -> Result<Option<u32>, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(&self.file).unwrap_or_default();
        let json: serde_json::Value = if contents.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&contents)?
        };
        if let Some(seed) = json.get("seed") {
            if let Some(seed) = seed.as_u64() {
                return Ok(Some(seed as u32));
            }
        }
        Ok(None)
    }
}