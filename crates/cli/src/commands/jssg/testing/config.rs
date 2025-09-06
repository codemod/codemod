use std::str::FromStr;

#[derive(Debug, Clone)]
pub enum ReporterType {
    Console,
    Json,
    Terse,
}

impl FromStr for ReporterType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "console" => Ok(ReporterType::Console),
            "json" => Ok(ReporterType::Json),
            "terse" => Ok(ReporterType::Terse),
            _ => Err(format!(
                "Invalid reporter type: {s}. Valid options: console, json, terse"
            )),
        }
    }
}
