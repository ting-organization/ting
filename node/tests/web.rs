use serde_json::Value;
use ting_node::{api, web};

fn check_refs(document: &Value, value: &Value) {
    match value {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref") {
                let pointer = reference
                    .as_str()
                    .unwrap()
                    .strip_prefix('#')
                    .expect("only local schema references");
                assert!(
                    document.pointer(pointer).is_some(),
                    "unresolved OpenAPI reference: {pointer}"
                );
            }
            for value in map.values() {
                check_refs(document, value);
            }
        }
        Value::Array(values) => {
            for value in values {
                check_refs(document, value);
            }
        }
        _ => {}
    }
}
#[test]
fn consumer_openapi_models_resolve_and_submit_example_is_canonical() {
    use ting_core::protocol::Transaction;
    let document = api::openapi();
    check_refs(&document, &document);
    let example =
        document["paths"]["/v1/transactions"]["post"]["requestBody"]["content"]["application/json"]
            ["example"]["hex"]
            .as_str()
            .unwrap();
    let bytes = ting_node::unhex(example, 8192).unwrap();
    assert!(Transaction::decode(&bytes, 8192).is_ok());
    assert!(
        document["components"]["schemas"]["TransactionLookup"]["properties"]
            .get("pending_transaction")
            .is_some()
    );
    assert!(
        document["components"]["schemas"]
            .get("Occurrence")
            .is_some()
    );
    for path in document["paths"].as_object().unwrap().values() {
        for method in path.as_object().unwrap().keys() {
            assert!(method == "get" || method == "post");
        }
    }
}
#[test]
fn vendored_swagger_assets_match_the_integrity_manifest() {
    use sha2::{Digest, Sha256};
    use ting_core::hash::hex;
    let manifest: Value =
        serde_json::from_str(include_str!("../vendor/swagger-ui/manifest.json")).unwrap();
    for (name, expected) in manifest["sha256"].as_object().unwrap() {
        let route = match name.as_str() {
            "LICENSE" => "swagger-ui-license.txt",
            "NOTICE" => "swagger-ui-notice.txt",
            other => other,
        };
        let (_, asset) = web::asset(route).unwrap();
        assert_eq!(
            hex(&Sha256::digest(asset.as_bytes())),
            expected.as_str().unwrap()
        );
    }
}
