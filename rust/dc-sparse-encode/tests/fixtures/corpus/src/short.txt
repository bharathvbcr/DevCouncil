/// Parses a JSON response from the HTTP server and returns its routes.
pub fn parse_json_response(body: &str) -> Result<Vec<Route>, Error> {
    let value: serde_json::Value = serde_json::from_str(body)?;
    value["routes"].as_array().map(|r| r.iter().map(Route::from).collect()).ok_or(Error::Shape)
}
