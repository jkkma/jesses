use media_core::{
    BatchEncodeRequest, BitrateRequest, EncodeRequest, QualityRequest, UtilityRequest,
};
use serde_json::{Value, json};

fn preview() -> Value {
    json!({
        "inputs": [{
            "inputPath": "source.mkv",
            "streamIndices": [0],
            "videoStreamIndex": 0
        }],
        "outputDirectory": "output",
        "crf": 30,
        "preset": 4
    })
}

fn encode() -> Value {
    json!({
        "source": {
            "inputPath": "source.mkv",
            "outputPath": "output.mkv",
            "streamIndices": [0]
        },
        "settings": {"videoStreamIndex": 0, "crf": 30, "preset": 4}
    })
}

fn non_encode_requests() -> Vec<Value> {
    vec![
        json!({"kind": "concat", "request": {
            "inputPaths": ["first.mkv", "second.mkv"], "outputPath": "joined.mkv"
        }}),
        json!({"referencePath": "reference.mkv", "candidatePath": "encoded.mkv",
            "referenceStreamIndex": 0, "candidateStreamIndex": 0,
            "referenceStartFrame": 0, "candidateStartFrame": 0,
            "frameCount": 24, "metric": "ssim"}),
        json!({"inputPath": "source.mkv", "streamIndex": 0, "windowSeconds": 1}),
        json!({"kind": "crfLadder", "request": {
            "inputPath": "source.mkv", "videoStreamIndex": 0, "encoder": "h264",
            "preset": "medium", "pixelFormat": "yuv420p", "crfs": [18, 23, 28],
            "sampleCount": 3, "sampleSeconds": 2, "metric": "ssim",
            "recommendationThreshold": null
        }}),
    ]
}

#[test]
fn non_encode_requests_remain_valid_for_their_own_commands() {
    let [concat, quality, bitrate, ladder]: [Value; 4] = non_encode_requests().try_into().unwrap();
    assert!(serde_json::from_value::<UtilityRequest>(concat).is_ok());
    assert!(serde_json::from_value::<QualityRequest>(quality).is_ok());
    assert!(serde_json::from_value::<BitrateRequest>(bitrate).is_ok());
    assert!(serde_json::from_value::<UtilityRequest>(ladder).is_ok());
}

#[test]
fn batch_contract_rejects_non_encode_requests_and_mixed_queue() {
    for request in non_encode_requests() {
        assert!(serde_json::from_value::<BatchEncodeRequest>(request.clone()).is_err());
        // Deserialization of the complete queue must fail, even when a valid
        // encode precedes the unsupported operation.
        assert!(serde_json::from_value::<Vec<EncodeRequest>>(json!([encode(), request])).is_err());
    }
}

#[test]
fn batch_contract_rejects_operation_fields_in_otherwise_valid_encodes() {
    for operation in non_encode_requests() {
        let mut mixed_preview = preview();
        let mut mixed_encode = encode();
        for (key, value) in operation.as_object().unwrap() {
            mixed_preview[key] = value.clone();
            mixed_encode[key] = value.clone();
        }
        assert!(
            serde_json::from_value::<BatchEncodeRequest>(mixed_preview).is_err(),
            "A batch preview must not silently discard operation fields: {operation}"
        );
        assert!(
            serde_json::from_value::<Vec<EncodeRequest>>(json!([encode(), mixed_encode])).is_err(),
            "An encode queue must not silently discard operation fields: {operation}"
        );
    }
}

#[test]
fn batch_contract_keeps_existing_encode_defaults_and_round_trips() {
    let preview: BatchEncodeRequest = serde_json::from_value(preview()).unwrap();
    let requests: Vec<EncodeRequest> = serde_json::from_value(json!([encode(), encode()])).unwrap();
    assert_eq!(preview.inputs.len(), 1);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].settings.crf, 30);
    assert_eq!(
        serde_json::from_value::<BatchEncodeRequest>(serde_json::to_value(&preview).unwrap())
            .unwrap(),
        preview
    );
    assert_eq!(
        serde_json::from_value::<Vec<EncodeRequest>>(serde_json::to_value(&requests).unwrap())
            .unwrap(),
        requests
    );
}
