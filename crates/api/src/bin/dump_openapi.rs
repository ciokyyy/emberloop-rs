use api::routes::ApiDoc;
use utoipa::OpenApi;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let json = ApiDoc::openapi().to_pretty_json()?;
    std::fs::write("openapi.json", format!("{json}\n"))?;
    Ok(())
}
