//! Schema-level checks that need no database.

use codeatlas_server::schema::{self, MAX_QUERY_DEPTH};

#[test]
fn committed_sdl_matches_the_schema() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/schema.graphql");
    let committed = std::fs::read_to_string(&path).unwrap();
    let generated = format!("{}\n", schema::build(None).sdl());
    assert!(
        committed == generated,
        "docs/schema.graphql is out of date; regenerate it with \
         `cargo run -p codeatlas-server -- --print-schema > docs/schema.graphql`"
    );
}

#[tokio::test]
async fn rejects_queries_nested_too_deep() {
    // Introspection types nest arbitrarily, so they can exceed the limit
    // without needing any data.
    let mut selection = String::from("name");
    for _ in 0..MAX_QUERY_DEPTH {
        selection = format!("ofType {{ {selection} }}");
    }
    let query = format!("{{ __schema {{ types {{ fields {{ type {{ {selection} }} }} }} }} }}");
    let response = schema::build(None).execute(query.as_str()).await;
    assert_eq!(response.errors.len(), 1);
    assert!(
        response.errors[0].message.contains("nested too deep"),
        "{}",
        response.errors[0].message
    );
}

#[tokio::test]
async fn rejects_overly_complex_queries() {
    // Many aliased copies of a small selection add up past the limit
    // (`__typename` is free, so real fields are used).
    let fields: String = (0..400)
        .map(|i| format!("a{i}: __schema {{ queryType {{ name }} }} "))
        .collect();
    let response = schema::build(None).execute(format!("{{ {fields} }}")).await;
    assert_eq!(response.errors.len(), 1);
    assert!(
        response.errors[0].message.contains("too complex"),
        "{}",
        response.errors[0].message
    );
}
