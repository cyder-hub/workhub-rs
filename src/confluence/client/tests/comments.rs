use super::support::*;
use super::*;

#[tokio::test]
async fn client_gets_page_comments_with_expanded_author_and_body() {
    let (base_url, requests) = mock_server(
        json!({
            "results": [{
                "id": "c-1",
                "type": "comment",
                "body": {"storage": {"value": "<p>First comment</p>"}},
                "version": {"number": 2, "by": {"displayName": "Ada"}},
                "container": {"id": "123", "type": "page", "title": "Roadmap"},
                "extensions": {"location": "footer"}
            }],
            "start": 0,
            "limit": 25,
            "size": 1
        }),
        StatusCode::OK,
    )
    .await;

    let response = client(base_url).get_page_comments("123").await.unwrap();

    assert_eq!(response.results.len(), 1);
    let simplified = response.results[0].to_simplified_value();
    assert_eq!(simplified["body"], json!("First comment"));
    assert_eq!(simplified["author"]["display_name"], json!("Ada"));
    assert_eq!(simplified["location"], json!("footer"));
    let requests = requests.lock().await;
    assert_eq!(requests[0].method, Method::GET);
    assert!(
        requests[0]
            .path
            .starts_with("/rest/api/content/123/child/comment?")
    );
    assert!(
        query_value(&requests[0].path, "expand")
            .unwrap()
            .contains("body.storage")
    );
    assert_eq!(
        query_value(&requests[0].path, "depth").as_deref(),
        Some("all")
    );
}

#[tokio::test]
async fn client_adds_and_replies_to_comments_with_storage_payloads() {
    let (base_url, requests) = queued_mock_server(vec![
        (
            StatusCode::OK,
            json!({
                "id": "c-1",
                "type": "comment",
                "body": {"storage": {"value": "<p>Comment</p>"}},
                "container": {"id": "123", "type": "page"}
            }),
        ),
        (
            StatusCode::OK,
            json!({
                "id": "c-2",
                "type": "comment",
                "body": {"storage": {"value": "<p>Reply</p>"}},
                "container": {"id": "c-1", "type": "comment"}
            }),
        ),
    ])
    .await;

    let first = client(base_url.clone())
        .add_comment("123", "<p>Comment</p>")
        .await
        .unwrap();
    let reply = client(base_url)
        .reply_to_comment("c-1", "<p>Reply</p>")
        .await
        .unwrap();

    assert_eq!(first.id.as_deref(), Some("c-1"));
    assert_eq!(reply.id.as_deref(), Some("c-2"));
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, Method::POST);
    assert_eq!(requests[0].path, "/rest/api/content");
    assert_eq!(requests[0].body["type"], json!("comment"));
    assert_eq!(requests[0].body["container"]["id"], json!("123"));
    assert_eq!(requests[0].body["container"]["type"], json!("page"));
    assert_eq!(
        requests[0].body["body"]["storage"]["value"],
        json!("<p>Comment</p>")
    );
    assert_eq!(requests[1].body["container"]["id"], json!("c-1"));
    assert_eq!(requests[1].body["container"]["type"], json!("comment"));
    assert_eq!(
        requests[1].body["body"]["storage"]["value"],
        json!("<p>Reply</p>")
    );
}

#[tokio::test]
async fn client_updates_comment_with_next_version_and_storage_payload() {
    let (base_url, requests) = queued_mock_server(vec![
        (
            StatusCode::OK,
            json!({
                "id": "c-1",
                "title": "Roadmap",
                "type": "comment",
                "body": {"storage": {"value": "<p>Old</p>"}},
                "version": {"number": 2},
                "container": {"id": "123", "type": "page", "title": "Roadmap"}
            }),
        ),
        (
            StatusCode::OK,
            json!({
                "id": "c-1",
                "title": "Roadmap",
                "type": "comment",
                "body": {"storage": {"value": "<p>Updated</p>"}},
                "version": {"number": 3},
                "container": {"id": "123", "type": "page", "title": "Roadmap"}
            }),
        ),
    ])
    .await;

    let updated = client(base_url)
        .update_comment("c-1", "<p>Updated</p>")
        .await
        .unwrap();

    assert_eq!(updated.id.as_deref(), Some("c-1"));
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, Method::GET);
    assert!(requests[0].path.starts_with("/rest/api/content/c-1?"));
    assert_eq!(requests[1].method, Method::PUT);
    assert_eq!(requests[1].path, "/rest/api/content/c-1");
    assert_eq!(requests[1].body["id"], json!("c-1"));
    assert_eq!(requests[1].body["type"], json!("comment"));
    assert_eq!(requests[1].body["title"], json!("Roadmap"));
    assert_eq!(requests[1].body["version"]["number"], json!(3));
    assert_eq!(requests[1].body["container"]["id"], json!("123"));
    assert_eq!(
        requests[1].body["body"]["storage"]["value"],
        json!("<p>Updated</p>")
    );
}

#[tokio::test]
async fn client_deletes_comment_by_content_id() {
    let (base_url, requests) = mock_server(json!({}), StatusCode::NO_CONTENT).await;

    client(base_url).delete_comment("c-1").await.unwrap();

    let requests = requests.lock().await;
    assert_eq!(requests[0].method, Method::DELETE);
    assert_eq!(requests[0].path, "/rest/api/content/c-1");
}
