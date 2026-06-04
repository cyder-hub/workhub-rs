use std::{fs, path::Path};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    atlassian::{
        error::AtlassianError,
        http::{AtlassianHttpClient, DownloadedContent},
    },
    confluence::{
        config::{ConfluenceConfig, ConfluenceDeployment},
        formatting::{body_value_as_storage, safe_path_segment},
        models::{
            ConfluenceAttachment, ConfluenceAttachmentListResponse, ConfluenceComment,
            ConfluenceCommentListResponse, ConfluenceLabelListResponse, ConfluencePage,
            ConfluencePageListResponse, ConfluencePageViews, ConfluenceSearchResponse,
            ConfluenceUserListResponse, ConfluenceUserSearchResponse, ConfluenceUserSearchResult,
        },
    },
};

pub const DEFAULT_LIMIT: u64 = 25;
pub const DEFAULT_ATTACHMENT_MAX_BYTES: u64 = 1_048_576;
pub const DEFAULT_ATTACHMENT_LIST_LIMIT: u64 = 50;
pub const MAX_ATTACHMENT_LIST_LIMIT: u64 = 100;
pub const DEFAULT_SEARCH_LIMIT: u64 = 10;
pub const MAX_SEARCH_LIMIT: u64 = 50;
pub const DEFAULT_USER_SEARCH_LIMIT: u64 = 10;
pub const MAX_USER_SEARCH_LIMIT: u64 = 50;
const SERVER_USER_SEARCH_PAGE_SIZE: u64 = 200;
const DEFAULT_CONFLUENCE_GROUP_NAME: &str = "confluence-users";

#[derive(Clone, Debug)]
pub struct ConfluenceClient {
    config: ConfluenceConfig,
    http: AtlassianHttpClient,
}

impl ConfluenceClient {
    pub fn new(config: ConfluenceConfig) -> Result<Self, AtlassianError> {
        let http = AtlassianHttpClient::new(
            &config.base_url,
            config.auth.clone(),
            config.timeout_seconds,
            config.ssl_verify,
        )?;
        Ok(Self { config, http })
    }

    pub fn config(&self) -> &ConfluenceConfig {
        &self.config
    }

    pub async fn get_page_by_id(
        &self,
        page_id: &str,
        expand: &[&str],
    ) -> Result<ConfluencePage, AtlassianError> {
        let page_id = safe_path_segment(page_id, "page_id")?;
        let mut query = Vec::new();
        if !expand.is_empty() {
            query.push(("expand".to_string(), expand.join(",")));
        }

        self.get_json(&format!("/rest/api/content/{page_id}"), query)
            .await
    }

    pub async fn get_page_history(
        &self,
        page_id: &str,
        version: u64,
        expand: &[&str],
    ) -> Result<ConfluencePage, AtlassianError> {
        if version == 0 {
            return Err(AtlassianError::invalid_input("version must be positive"));
        }
        let page_id = safe_path_segment(page_id, "page_id")?;
        let mut query = vec![
            ("status".to_string(), "historical".to_string()),
            ("version".to_string(), version.to_string()),
        ];
        if !expand.is_empty() {
            query.push(("expand".to_string(), expand.join(",")));
        }

        self.get_json(&format!("/rest/api/content/{page_id}"), query)
            .await
    }

    pub async fn get_page_by_title(
        &self,
        space_key: &str,
        title: &str,
        expand: &[&str],
    ) -> Result<Option<ConfluencePage>, AtlassianError> {
        let space_key = required_non_empty_input(space_key, "space_key")?;
        let title = required_non_empty_input(title, "title")?;
        let mut query = vec![
            ("spaceKey".to_string(), space_key),
            ("title".to_string(), title),
            ("limit".to_string(), "1".to_string()),
        ];
        if !expand.is_empty() {
            query.push(("expand".to_string(), expand.join(",")));
        }

        let response: ConfluencePageListResponse =
            self.get_json("/rest/api/content", query).await?;
        Ok(response.results.into_iter().next())
    }

    pub async fn get_page_children_by_type(
        &self,
        page_id: &str,
        child_type: &str,
        start: Option<u64>,
        limit: Option<u64>,
        expand: &[&str],
    ) -> Result<ConfluencePageListResponse, AtlassianError> {
        let page_id = safe_path_segment(page_id, "page_id")?;
        let child_type = safe_path_segment(child_type, "child_type")?;
        let mut query = vec![
            ("start".to_string(), start.unwrap_or(0).to_string()),
            (
                "limit".to_string(),
                limit.unwrap_or(DEFAULT_LIMIT).to_string(),
            ),
        ];
        if !expand.is_empty() {
            query.push(("expand".to_string(), expand.join(",")));
        }

        self.get_json(
            &format!("/rest/api/content/{page_id}/child/{child_type}"),
            query,
        )
        .await
    }

    pub async fn get_page_children(
        &self,
        page_id: &str,
        start: Option<u64>,
        limit: Option<u64>,
        expand: &[&str],
        include_folders: bool,
    ) -> Result<Vec<ConfluencePage>, AtlassianError> {
        let mut children = self
            .get_page_children_by_type(page_id, "page", start, limit, expand)
            .await?
            .results;

        if include_folders
            && let Ok(folder_response) = self
                .get_page_children_by_type(page_id, "folder", start, limit, expand)
                .await
        {
            children.extend(folder_response.results);
        }

        Ok(children)
    }

    pub async fn get_space_pages(
        &self,
        space_key: &str,
        start: Option<u64>,
        limit: Option<u64>,
        expand: &[&str],
    ) -> Result<ConfluencePageListResponse, AtlassianError> {
        let space_key = required_non_empty_input(space_key, "space_key")?;
        let mut query = vec![
            ("spaceKey".to_string(), space_key),
            ("type".to_string(), "page".to_string()),
            ("start".to_string(), start.unwrap_or(0).to_string()),
            (
                "limit".to_string(),
                limit.unwrap_or(DEFAULT_LIMIT).to_string(),
            ),
        ];
        if !expand.is_empty() {
            query.push(("expand".to_string(), expand.join(",")));
        }

        self.get_json("/rest/api/content", query).await
    }

    pub async fn get_page_comments(
        &self,
        page_id: &str,
    ) -> Result<ConfluenceCommentListResponse, AtlassianError> {
        let page_id = safe_path_segment(page_id, "page_id")?;
        self.get_json(
            &format!("/rest/api/content/{page_id}/child/comment"),
            vec![
                (
                    "expand".to_string(),
                    "body.storage,body.view,version,container,ancestors,extensions".to_string(),
                ),
                ("depth".to_string(), "all".to_string()),
            ],
        )
        .await
    }

    pub async fn add_comment(
        &self,
        page_id: &str,
        storage_body: &str,
    ) -> Result<ConfluenceComment, AtlassianError> {
        let payload = comment_payload(page_id, "page", storage_body)?;
        self.http
            .send_json(self.http.post_json("/rest/api/content", &payload)?)
            .await
    }

    pub async fn reply_to_comment(
        &self,
        comment_id: &str,
        storage_body: &str,
    ) -> Result<ConfluenceComment, AtlassianError> {
        let payload = comment_payload(comment_id, "comment", storage_body)?;
        self.http
            .send_json(self.http.post_json("/rest/api/content", &payload)?)
            .await
    }

    pub async fn get_labels(
        &self,
        content_id: &str,
    ) -> Result<ConfluenceLabelListResponse, AtlassianError> {
        let content_id = safe_path_segment(content_id, "content_id")?;
        self.get_json(&format!("/rest/api/content/{content_id}/label"), Vec::new())
            .await
    }

    pub async fn add_label(
        &self,
        content_id: &str,
        name: &str,
    ) -> Result<ConfluenceLabelListResponse, AtlassianError> {
        let content_id = safe_path_segment(content_id, "content_id")?;
        let name = required_non_empty_input(name, "name")?;
        let payload = json!([{ "prefix": "global", "name": name }]);

        self.http
            .send_json_value_or_null(
                self.http
                    .post_json(&format!("/rest/api/content/{content_id}/label"), &payload)?,
            )
            .await?;
        self.get_labels(&content_id).await
    }

    pub async fn search_user(
        &self,
        cql: &str,
        limit: Option<u64>,
        group_name: Option<&str>,
    ) -> Result<ConfluenceUserSearchResponse, AtlassianError> {
        let cql = required_non_empty_input(cql, "query")?;
        let limit = user_search_limit(limit)?;

        match self.config.deployment {
            ConfluenceDeployment::Cloud => {
                self.get_json(
                    "/rest/api/search/user",
                    vec![
                        ("cql".to_string(), cql),
                        ("limit".to_string(), limit.to_string()),
                    ],
                )
                .await
            }
            ConfluenceDeployment::ServerDataCenter => {
                self.search_user_server_dc(&cql, group_name, limit).await
            }
        }
    }

    async fn search_user_server_dc(
        &self,
        cql: &str,
        group_name: Option<&str>,
        limit: u64,
    ) -> Result<ConfluenceUserSearchResponse, AtlassianError> {
        let group_name = required_non_empty_input(
            group_name.unwrap_or(DEFAULT_CONFLUENCE_GROUP_NAME),
            "group_name",
        )?;
        let search_term = extract_user_fullname_search_term(cql).unwrap_or(cql);
        let search_lower = search_term.to_ascii_lowercase();
        let mut start = 0;
        let mut matches = Vec::new();

        while matches.len() < limit as usize {
            let encoded_group = percent_encode_path_segment(&group_name);
            let response: ConfluenceUserListResponse = self
                .get_json(
                    &format!("/rest/api/group/{encoded_group}/member"),
                    vec![
                        ("start".to_string(), start.to_string()),
                        (
                            "limit".to_string(),
                            SERVER_USER_SEARCH_PAGE_SIZE.to_string(),
                        ),
                    ],
                )
                .await?;
            let member_count = response.results.len() as u64;

            for user in response.results {
                let display_name = user.display_name.clone().unwrap_or_default();
                let username = user.username.clone().unwrap_or_default();
                if display_name.to_ascii_lowercase().contains(&search_lower)
                    || username.to_ascii_lowercase().contains(&search_lower)
                {
                    matches.push(ConfluenceUserSearchResult::from_user(
                        user,
                        Some(display_name),
                    ));
                    if matches.len() >= limit as usize {
                        break;
                    }
                }
            }

            if member_count == 0 || response.links.get("next").is_none() {
                break;
            }
            start += member_count;
        }

        Ok(ConfluenceUserSearchResponse {
            start: Some(0),
            limit: Some(limit),
            size: Some(matches.len() as u64),
            total_size: Some(matches.len() as u64),
            cql_query: Some(cql.to_string()),
            results: matches,
            ..ConfluenceUserSearchResponse::default()
        })
    }

    pub async fn get_page_views(
        &self,
        page_id: &str,
        include_title: bool,
    ) -> Result<ConfluencePageViews, AtlassianError> {
        if self.config.deployment != ConfluenceDeployment::Cloud {
            return Err(AtlassianError::invalid_input(
                "Page view analytics is only available for Confluence Cloud. Server/Data Center instances do not support the Analytics API.",
            ));
        }
        let page_id = safe_path_segment(page_id, "page_id")?;
        let title = if include_title {
            self.get_page_by_id(&page_id, &["title"])
                .await
                .ok()
                .and_then(|page| page.title)
        } else {
            None
        };
        let mut views: ConfluencePageViews = self
            .get_json(
                &format!("/rest/api/analytics/content/{page_id}/views"),
                Vec::new(),
            )
            .await?;
        views.page_id = Some(page_id);
        views.title = title;

        Ok(views)
    }

    pub async fn get_attachments(
        &self,
        content_id: &str,
        start: Option<u64>,
        limit: Option<u64>,
        filename: Option<&str>,
        media_type: Option<&str>,
    ) -> Result<ConfluenceAttachmentListResponse, AtlassianError> {
        let content_id = safe_path_segment(content_id, "content_id")?;
        let start = start.unwrap_or(0);
        let limit = attachment_list_limit(limit)?;
        let filename = optional_non_empty_input(filename);
        let media_type = optional_non_empty_input(media_type);
        let mut response: ConfluenceAttachmentListResponse = self
            .get_json(
                &format!("/rest/api/content/{content_id}/child/attachment"),
                vec![
                    ("start".to_string(), start.to_string()),
                    ("limit".to_string(), limit.to_string()),
                    (
                        "expand".to_string(),
                        "metadata,extensions,version".to_string(),
                    ),
                ],
            )
            .await?;

        if filename.is_some() || media_type.is_some() {
            response.results.retain(|attachment| {
                filename
                    .as_deref()
                    .is_none_or(|value| attachment.title.as_deref() == Some(value))
                    && media_type
                        .as_deref()
                        .is_none_or(|value| attachment.media_type() == Some(value))
            });
            response.size = Some(response.results.len() as u64);
        }

        Ok(response)
    }

    pub async fn get_attachment_by_id(
        &self,
        attachment_id: &str,
    ) -> Result<crate::confluence::models::ConfluenceAttachment, AtlassianError> {
        let attachment_id = safe_path_segment(attachment_id, "attachment_id")?;
        self.get_json(
            &format!("/rest/api/content/{attachment_id}"),
            vec![(
                "expand".to_string(),
                "metadata,extensions,version".to_string(),
            )],
        )
        .await
    }

    pub async fn upload_attachment(
        &self,
        content_id: &str,
        file_path: &str,
        comment: Option<&str>,
        minor_edit: bool,
    ) -> Result<ConfluenceAttachment, AtlassianError> {
        let content_id = safe_path_segment(content_id, "content_id")?;
        let file_path = required_non_empty_input(file_path, "file_path")?;
        let path = Path::new(&file_path);
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AtlassianError::invalid_input("file_path must include a filename"))?
            .to_string();
        let bytes = fs::read(path).map_err(|error| {
            AtlassianError::invalid_input(format!(
                "failed to read local file `{filename}`: {error}"
            ))
        })?;
        let (body, content_type) =
            attachment_multipart_body(&filename, &bytes, comment, minor_edit);
        let value = self
            .http
            .send_json_value_or_null(self.http.put_body_with_headers(
                &format!("/rest/api/content/{content_id}/child/attachment"),
                body,
                &content_type,
                &[("x-atlassian-token", "nocheck")],
            )?)
            .await?;

        confluence_attachment_from_upload_response(value)
    }

    pub async fn delete_attachment(
        &self,
        attachment_id: &str,
    ) -> Result<serde_json::Value, AtlassianError> {
        let attachment_id = safe_path_segment(attachment_id, "attachment_id")?;
        self.http
            .send_json_value_or_null(
                self.http
                    .delete(&format!("/rest/api/content/{attachment_id}"))?,
            )
            .await
    }

    pub async fn create_page(
        &self,
        space_key: &str,
        title: &str,
        storage_body: &str,
        parent_id: Option<&str>,
    ) -> Result<ConfluencePage, AtlassianError> {
        let payload = page_write_payload(
            None,
            required_non_empty_input(space_key, "space_key")?,
            required_non_empty_input(title, "title")?,
            storage_body,
            parent_id,
            None,
            None,
        )?;

        self.http
            .send_json(self.http.post_json("/rest/api/content", &payload)?)
            .await
    }

    pub async fn update_page(
        &self,
        page_id: &str,
        title: &str,
        storage_body: &str,
        parent_id: Option<&str>,
        is_minor_edit: bool,
        version_comment: Option<&str>,
    ) -> Result<ConfluencePage, AtlassianError> {
        let current = self
            .get_page_by_id(page_id, &["version", "space", "body.storage"])
            .await?;
        let space_key = current
            .space
            .as_ref()
            .and_then(|space| space.key.as_deref())
            .ok_or_else(|| {
                AtlassianError::unexpected_shape("page response is missing space.key")
            })?;
        let next_version = current
            .version
            .as_ref()
            .and_then(|version| version.number)
            .unwrap_or(1)
            + 1;

        self.update_page_with_space(
            page_id,
            space_key,
            title,
            storage_body,
            parent_id,
            next_version,
            is_minor_edit,
            version_comment,
        )
        .await
    }

    pub async fn delete_page(&self, page_id: &str) -> Result<serde_json::Value, AtlassianError> {
        let page_id = safe_path_segment(page_id, "page_id")?;
        self.http
            .send_json_value_or_null(self.http.delete(&format!("/rest/api/content/{page_id}"))?)
            .await
    }

    pub async fn set_page_emoji_best_effort(&self, page_id: &str, emoji: Option<&str>) {
        let Some(emoji) = optional_non_empty_input(emoji) else {
            return;
        };
        let Ok(page_id) = safe_path_segment(page_id, "page_id") else {
            return;
        };
        let payload = json!({ "value": emoji });
        let Ok(builder) = self.http.put_json(
            &format!("/rest/api/content/{page_id}/property/emoji-title-published"),
            &payload,
        ) else {
            return;
        };

        let _ = self.http.send_json_value_or_null(builder).await;
    }

    pub async fn move_page(
        &self,
        page_id: &str,
        target_parent_id: Option<&str>,
        target_space_key: Option<&str>,
        position: Option<&str>,
    ) -> Result<ConfluencePage, AtlassianError> {
        let page_id = safe_path_segment(page_id, "page_id")?;
        let target_parent_id = optional_non_empty_input(target_parent_id);
        let target_space_key = optional_non_empty_input(target_space_key);
        let position = position
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("append");
        if !matches!(position, "append" | "above" | "below") {
            return Err(AtlassianError::invalid_input(
                "position must be append, above, or below",
            ));
        }
        if target_parent_id.is_none() && target_space_key.is_none() {
            return Err(AtlassianError::invalid_input(
                "At least one of target_parent_id or target_space_key must be provided",
            ));
        }

        if matches!(position, "above" | "below") {
            let target_id = target_parent_id.ok_or_else(|| {
                AtlassianError::invalid_input(
                    "target_parent_id is required when position is above or below",
                )
            })?;
            let target_id = safe_path_segment(&target_id, "target_parent_id")?;
            self.http
                .send_json_value_or_null(self.http.put_json(
                    &format!("/rest/api/content/{page_id}/move/{position}/{target_id}"),
                    &serde_json::json!({}),
                )?)
                .await?;
            return self
                .get_page_by_id(&page_id, &["body.storage", "version", "space", "ancestors"])
                .await;
        }

        let current = self
            .get_page_by_id(&page_id, &["body.storage", "version", "space"])
            .await?;
        let title = current
            .title
            .as_deref()
            .ok_or_else(|| AtlassianError::unexpected_shape("page response is missing title"))?;
        let storage_body = body_value_as_storage(&current.body).unwrap_or_default();
        let current_space_key = current
            .space
            .as_ref()
            .and_then(|space| space.key.as_deref())
            .ok_or_else(|| {
                AtlassianError::unexpected_shape("page response is missing space.key")
            })?;
        let space_key = target_space_key.as_deref().unwrap_or(current_space_key);
        let next_version = current
            .version
            .as_ref()
            .and_then(|version| version.number)
            .unwrap_or(1)
            + 1;

        self.update_page_with_space(
            &page_id,
            space_key,
            title,
            &storage_body,
            target_parent_id.as_deref(),
            next_version,
            false,
            None,
        )
        .await
    }

    async fn update_page_with_space(
        &self,
        page_id: &str,
        space_key: &str,
        title: &str,
        storage_body: &str,
        parent_id: Option<&str>,
        version: u64,
        is_minor_edit: bool,
        version_comment: Option<&str>,
    ) -> Result<ConfluencePage, AtlassianError> {
        let page_id = safe_path_segment(page_id, "page_id")?;
        let payload = page_write_payload(
            Some(page_id.as_str()),
            required_non_empty_input(space_key, "space_key")?,
            required_non_empty_input(title, "title")?,
            storage_body,
            parent_id,
            Some(version),
            Some((is_minor_edit, version_comment)),
        )?;

        self.http
            .send_json(
                self.http
                    .put_json(&format!("/rest/api/content/{page_id}"), &payload)?,
            )
            .await
    }

    pub async fn search_cql(
        &self,
        cql: &str,
        start: Option<u64>,
        limit: Option<u64>,
    ) -> Result<ConfluenceSearchResponse, AtlassianError> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        let start = start.unwrap_or(0);
        self.get_json(
            "/rest/api/content/search",
            vec![
                ("cql".to_string(), cql.to_string()),
                ("start".to_string(), start.to_string()),
                ("limit".to_string(), limit.to_string()),
            ],
        )
        .await
    }

    pub async fn search_content(
        &self,
        query: &str,
        limit: Option<u64>,
        spaces_filter: Option<&str>,
    ) -> Result<ConfluenceSearchResponse, AtlassianError> {
        let query = required_non_empty_input(query, "query")?;
        let limit = search_limit(limit)?;

        if is_simple_search_query(&query) {
            let escaped_query = escape_cql_string(&query);
            let site_search_cql = format!("siteSearch ~ \"{escaped_query}\"");

            return match self
                .search_cql_with_spaces_filter(&site_search_cql, Some(limit), spaces_filter)
                .await
            {
                Ok(response) => Ok(response),
                Err(AtlassianError::HttpStatus { status: 400, .. }) => {
                    let text_search_cql = format!("text ~ \"{escaped_query}\"");
                    self.search_cql_with_spaces_filter(&text_search_cql, Some(limit), spaces_filter)
                        .await
                }
                Err(error) => Err(error),
            };
        }

        self.search_cql_with_spaces_filter(&query, Some(limit), spaces_filter)
            .await
    }

    async fn search_cql_with_spaces_filter(
        &self,
        cql: &str,
        limit: Option<u64>,
        spaces_filter: Option<&str>,
    ) -> Result<ConfluenceSearchResponse, AtlassianError> {
        let cql = apply_spaces_filter(cql, spaces_filter_values(spaces_filter, &self.config));
        self.search_cql(&cql, Some(0), limit).await
    }

    pub async fn get_json<T>(
        &self,
        path: &str,
        query: Vec<(String, String)>,
    ) -> Result<T, AtlassianError>
    where
        T: DeserializeOwned,
    {
        let builder = self.http.get(path)?.query(&query);
        self.http.send_json(builder).await
    }

    pub async fn download_relative_or_same_origin(
        &self,
        url: &str,
        max_bytes: u64,
    ) -> Result<DownloadedContent, AtlassianError> {
        let builder = self
            .http
            .get_same_origin_or_relative_url(url, "download_url")?;
        self.http.send_bytes_limited(builder, max_bytes).await
    }
}

fn page_write_payload(
    page_id: Option<&str>,
    space_key: String,
    title: String,
    storage_body: &str,
    parent_id: Option<&str>,
    version: Option<u64>,
    version_options: Option<(bool, Option<&str>)>,
) -> Result<Value, AtlassianError> {
    let mut payload = json!({
        "type": "page",
        "title": title,
        "space": {"key": space_key},
        "body": {
            "storage": {
                "value": storage_body,
                "representation": "storage"
            }
        }
    });

    if let Some(page_id) = page_id {
        payload["id"] = Value::String(page_id.to_string());
    }
    if let Some(parent_id) = optional_non_empty_input(parent_id) {
        payload["ancestors"] = json!([{ "id": parent_id }]);
    }
    if let Some(version) = version {
        let (minor_edit, message) = version_options.unwrap_or((false, None));
        payload["version"] = json!({
            "number": version,
            "minorEdit": minor_edit,
        });
        if let Some(message) = optional_non_empty_input(message) {
            payload["version"]["message"] = Value::String(message);
        }
    }

    Ok(payload)
}

fn comment_payload(
    container_id: &str,
    container_type: &'static str,
    storage_body: &str,
) -> Result<Value, AtlassianError> {
    let container_id = safe_path_segment(container_id, "container_id")?;
    Ok(json!({
        "type": "comment",
        "container": {
            "id": container_id,
            "type": container_type,
        },
        "body": {
            "storage": {
                "value": storage_body,
                "representation": "storage"
            }
        }
    }))
}

fn required_non_empty_input(
    value: &str,
    field_name: &'static str,
) -> Result<String, AtlassianError> {
    let value = value.trim();
    if value.is_empty() {
        Err(AtlassianError::invalid_input(format!(
            "{field_name} must not be empty"
        )))
    } else {
        Ok(value.to_string())
    }
}

fn optional_non_empty_input(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn search_limit(value: Option<u64>) -> Result<u64, AtlassianError> {
    match value.unwrap_or(DEFAULT_SEARCH_LIMIT) {
        0 => Err(AtlassianError::invalid_input("limit must be positive")),
        value if value > MAX_SEARCH_LIMIT => Err(AtlassianError::invalid_input(format!(
            "limit must be less than or equal to {MAX_SEARCH_LIMIT}"
        ))),
        value => Ok(value),
    }
}

fn user_search_limit(value: Option<u64>) -> Result<u64, AtlassianError> {
    match value.unwrap_or(DEFAULT_USER_SEARCH_LIMIT) {
        0 => Err(AtlassianError::invalid_input("limit must be positive")),
        value if value > MAX_USER_SEARCH_LIMIT => Err(AtlassianError::invalid_input(format!(
            "limit must be less than or equal to {MAX_USER_SEARCH_LIMIT}"
        ))),
        value => Ok(value),
    }
}

fn attachment_list_limit(value: Option<u64>) -> Result<u64, AtlassianError> {
    match value.unwrap_or(DEFAULT_ATTACHMENT_LIST_LIMIT) {
        0 => Err(AtlassianError::invalid_input("limit must be positive")),
        value if value > MAX_ATTACHMENT_LIST_LIMIT => Err(AtlassianError::invalid_input(format!(
            "limit must be less than or equal to {MAX_ATTACHMENT_LIST_LIMIT}"
        ))),
        value => Ok(value),
    }
}

fn attachment_multipart_body(
    filename: &str,
    file_bytes: &[u8],
    comment: Option<&str>,
    minor_edit: bool,
) -> (Vec<u8>, String) {
    let boundary = "mcp-atlassian-rs-boundary";
    let mut body = Vec::with_capacity(file_bytes.len() + 512);
    let filename = multipart_header_value(filename);

    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(b"\r\n");

    if let Some(comment) = comment.map(str::trim).filter(|value| !value.is_empty()) {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"comment\"\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(comment.as_bytes());
        body.extend_from_slice(b"\r\n");
    }

    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"minorEdit\"\r\n\r\n{}\r\n",
            if minor_edit { "true" } else { "false" }
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    (body, format!("multipart/form-data; boundary={boundary}"))
}

fn multipart_header_value(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '"' | '\\' | '\r' | '\n' => '_',
            value => value,
        })
        .collect()
}

fn confluence_attachment_from_upload_response(
    value: Value,
) -> Result<ConfluenceAttachment, AtlassianError> {
    let attachment = value
        .get("results")
        .and_then(Value::as_array)
        .and_then(|results| results.first())
        .cloned()
        .unwrap_or(value);
    if attachment.is_null() {
        return Err(AtlassianError::unexpected_shape(
            "attachment upload response is missing an attachment result",
        ));
    }

    serde_json::from_value(attachment).map_err(|error| {
        AtlassianError::json_decode_body(error, Some("Confluence attachment upload response"))
    })
}

fn is_simple_search_query(query: &str) -> bool {
    !["=", "~", ">", "<", " AND ", " OR ", "currentUser()"]
        .iter()
        .any(|token| query.contains(token))
}

fn escape_cql_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn extract_user_fullname_search_term(cql: &str) -> Option<&str> {
    let marker = "user.fullname";
    let marker_index = cql.find(marker)?;
    let after_marker = &cql[marker_index + marker.len()..];
    let tilde_index = after_marker.find('~')?;
    let after_tilde = after_marker[tilde_index + 1..].trim_start();
    let quoted = after_tilde.strip_prefix('"')?;
    let end = quoted.find('"')?;
    Some(&quoted[..end])
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn spaces_filter_values(spaces_filter: Option<&str>, config: &ConfluenceConfig) -> Vec<String> {
    match spaces_filter {
        Some(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        None => config.spaces_filter.iter().cloned().collect(),
    }
}

fn apply_spaces_filter(cql: &str, spaces: Vec<String>) -> String {
    if spaces.is_empty() || cql_contains_space_filter(cql) {
        return cql.to_string();
    }

    let space_query = spaces
        .iter()
        .map(|space| format!("space = {}", quote_cql_identifier_if_needed(space)))
        .collect::<Vec<_>>()
        .join(" OR ");

    if cql.trim().is_empty() {
        space_query
    } else {
        format!("({cql}) AND ({space_query})")
    }
}

fn cql_contains_space_filter(cql: &str) -> bool {
    let normalized = cql.to_ascii_lowercase().replace(char::is_whitespace, "");
    normalized.contains("space=")
}

fn quote_cql_identifier_if_needed(identifier: &str) -> String {
    let starts_with_digit = identifier
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit());
    let has_special_character = !identifier
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_');
    let is_reserved = matches!(
        identifier.to_ascii_lowercase().as_str(),
        "and" | "or" | "not" | "in" | "order" | "by" | "space"
    );

    if identifier.starts_with('~') || starts_with_digit || has_special_character || is_reserved {
        format!("\"{}\"", escape_cql_string(identifier))
    } else {
        identifier.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeSet, VecDeque},
        net::SocketAddr,
        sync::Arc,
    };

    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, Method, StatusCode},
        response::{IntoResponse, Response},
        routing::any,
    };
    use serde_json::{Value, json};
    use tokio::sync::Mutex;

    use crate::{
        atlassian::{auth::AtlassianAuth, error::AtlassianError},
        confluence::config::{ConfluenceConfig, ConfluenceDeployment},
    };

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct RecordedRequest {
        method: Method,
        path: String,
        authorization: Option<String>,
        body: Value,
    }

    #[derive(Clone)]
    struct MockState {
        response: Value,
        status: StatusCode,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    #[derive(Clone)]
    struct QueuedMockState {
        responses: Arc<Mutex<VecDeque<(StatusCode, Value)>>>,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    async fn mock_handler(
        State(state): State<MockState>,
        method: Method,
        headers: HeaderMap,
        uri: axum::http::Uri,
        body: Bytes,
    ) -> Response {
        record_request(state.requests.clone(), method, headers, uri, body).await;
        (state.status, Json(state.response)).into_response()
    }

    async fn invalid_json_handler(
        State(requests): State<Arc<Mutex<Vec<RecordedRequest>>>>,
        method: Method,
        headers: HeaderMap,
        uri: axum::http::Uri,
        body: Bytes,
    ) -> Response {
        record_request(requests, method, headers, uri, body).await;
        (StatusCode::OK, "not-json").into_response()
    }

    async fn queued_mock_handler(
        State(state): State<QueuedMockState>,
        method: Method,
        headers: HeaderMap,
        uri: axum::http::Uri,
        body: Bytes,
    ) -> Response {
        record_request(state.requests.clone(), method, headers, uri, body).await;
        let (status, response) = {
            let mut responses = state.responses.lock().await;
            if responses.len() > 1 {
                responses.pop_front().unwrap()
            } else {
                responses
                    .front()
                    .cloned()
                    .unwrap_or((StatusCode::OK, json!({})))
            }
        };

        (status, Json(response)).into_response()
    }

    async fn record_request(
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        method: Method,
        headers: HeaderMap,
        uri: axum::http::Uri,
        body: Bytes,
    ) {
        let parsed_body = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        requests.lock().await.push(RecordedRequest {
            method,
            path: uri
                .path_and_query()
                .map(ToString::to_string)
                .unwrap_or_else(|| uri.path().to_string()),
            authorization: headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body: parsed_body,
        });
    }

    async fn mock_server(
        response: Value,
        status: StatusCode,
    ) -> (String, Arc<Mutex<Vec<RecordedRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = MockState {
            response,
            status,
            requests: requests.clone(),
        };
        let app = Router::new().fallback(any(mock_handler)).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{address}"), requests)
    }

    async fn invalid_json_mock_server() -> (String, Arc<Mutex<Vec<RecordedRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .fallback(any(invalid_json_handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{address}"), requests)
    }

    async fn queued_mock_server(
        responses: Vec<(StatusCode, Value)>,
    ) -> (String, Arc<Mutex<Vec<RecordedRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .fallback(any(queued_mock_handler))
            .with_state(QueuedMockState {
                responses: Arc::new(Mutex::new(VecDeque::from(responses))),
                requests: requests.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{address}"), requests)
    }

    fn client(base_url: String) -> ConfluenceClient {
        client_with_spaces_filter(base_url, BTreeSet::new())
    }

    fn cloud_client(base_url: String) -> ConfluenceClient {
        ConfluenceClient::new(ConfluenceConfig {
            base_url,
            deployment: ConfluenceDeployment::Cloud,
            auth: AtlassianAuth::Basic {
                username: "test-user".to_string(),
                api_token: "test-api-token".to_string(),
            },
            ssl_verify: true,
            spaces_filter: BTreeSet::new(),
            timeout_seconds: 75,
        })
        .unwrap()
    }

    fn client_with_spaces_filter(
        base_url: String,
        spaces_filter: BTreeSet<String>,
    ) -> ConfluenceClient {
        ConfluenceClient::new(ConfluenceConfig {
            base_url,
            deployment: ConfluenceDeployment::ServerDataCenter,
            auth: AtlassianAuth::Pat {
                personal_token: "test-pat-value".to_string(),
            },
            ssl_verify: true,
            spaces_filter,
            timeout_seconds: 75,
        })
        .unwrap()
    }

    fn query_value(path: &str, key: &str) -> Option<String> {
        let url = reqwest::Url::parse(&format!("http://example{path}")).unwrap();
        url.query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
    }

    #[tokio::test]
    async fn client_preserves_wiki_base_path_when_building_rest_url() {
        let (base_url, requests) = mock_server(
            json!({
                "id": "123",
                "title": "Roadmap",
                "body": {"storage": {"value": "<p>Hello</p>"}}
            }),
            StatusCode::OK,
        )
        .await;
        let client = client(format!("{base_url}/wiki"));
        let page = client
            .get_page_by_id("123", &["body.storage", "version", "space"])
            .await
            .unwrap();

        assert_eq!(page.id.as_deref(), Some("123"));
        let requests = requests.lock().await;
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(
            requests[0].path,
            "/wiki/rest/api/content/123?expand=body.storage%2Cversion%2Cspace"
        );
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer test-pat-value")
        );
    }

    #[tokio::test]
    async fn client_maps_http_status_errors_without_echoing_body() {
        let (base_url, _requests) = mock_server(
            json!({"errorMessages": ["page not found"]}),
            StatusCode::NOT_FOUND,
        )
        .await;
        let error = client(base_url)
            .get_page_by_id("missing", &[])
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AtlassianError::HttpStatus { status: 404, .. }
        ));
        assert!(error.to_string().contains("page not found"));
    }

    #[tokio::test]
    async fn client_maps_invalid_json_with_request_context() {
        let (base_url, _requests) = invalid_json_mock_server().await;
        let error = client(base_url)
            .get_page_by_id("123", &[])
            .await
            .unwrap_err();

        assert!(matches!(error, AtlassianError::JsonDecode { .. }));
        assert!(error.to_string().contains("GET /rest/api/content/123"));
    }

    #[tokio::test]
    async fn client_parses_missing_fields_without_failure() {
        let (base_url, _requests) = mock_server(json!({}), StatusCode::OK).await;
        let page = client(base_url).get_page_by_id("123", &[]).await.unwrap();

        assert_eq!(page.id, None);
        assert_eq!(page.title, None);
        assert!(page.body.is_null());
    }

    #[tokio::test]
    async fn client_gets_page_by_title_and_space_key() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [{
                    "id": "123",
                    "title": "Roadmap",
                    "space": {"key": "ENG"},
                    "body": {"storage": {"value": "<p>Hello</p>"}}
                }],
                "start": 0,
                "limit": 1,
                "size": 1
            }),
            StatusCode::OK,
        )
        .await;
        let page = client(base_url)
            .get_page_by_title("ENG", "Roadmap", &["body.storage", "version", "space"])
            .await
            .unwrap()
            .unwrap();

        assert_eq!(page.id.as_deref(), Some("123"));
        assert_eq!(page.title.as_deref(), Some("Roadmap"));
        let requests = requests.lock().await;
        assert_eq!(requests[0].method, Method::GET);
        assert!(requests[0].path.starts_with("/rest/api/content?"));
        assert_eq!(
            query_value(&requests[0].path, "spaceKey").as_deref(),
            Some("ENG")
        );
        assert_eq!(
            query_value(&requests[0].path, "title").as_deref(),
            Some("Roadmap")
        );
        assert_eq!(
            query_value(&requests[0].path, "limit").as_deref(),
            Some("1")
        );
    }

    #[tokio::test]
    async fn client_gets_specific_page_history_version() {
        let (base_url, requests) = mock_server(
            json!({
                "id": "123",
                "title": "Roadmap",
                "status": "historical",
                "space": {"key": "ENG"},
                "version": {"number": 2},
                "body": {"storage": {"value": "<p>Version two</p>"}}
            }),
            StatusCode::OK,
        )
        .await;

        let page = client(base_url)
            .get_page_history("123", 2, &["body.storage", "version", "space"])
            .await
            .unwrap();

        assert_eq!(page.version.and_then(|version| version.number), Some(2));
        let requests = requests.lock().await;
        assert_eq!(requests[0].method, Method::GET);
        assert!(requests[0].path.starts_with("/rest/api/content/123?"));
        assert_eq!(
            query_value(&requests[0].path, "status").as_deref(),
            Some("historical")
        );
        assert_eq!(
            query_value(&requests[0].path, "version").as_deref(),
            Some("2")
        );
        assert_eq!(
            query_value(&requests[0].path, "expand").as_deref(),
            Some("body.storage,version,space")
        );
    }

    #[tokio::test]
    async fn client_rejects_zero_page_history_version_before_http() {
        let (base_url, requests) = mock_server(json!({}), StatusCode::OK).await;
        let error = client(base_url)
            .get_page_history("123", 0, &[])
            .await
            .unwrap_err();

        assert!(error.to_string().contains("version must be positive"));
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn client_get_page_by_title_returns_none_for_empty_results() {
        let (base_url, _requests) = mock_server(json!({"results": []}), StatusCode::OK).await;

        let page = client(base_url)
            .get_page_by_title("ENG", "Missing", &["body.storage"])
            .await
            .unwrap();

        assert_eq!(page, None);
    }

    #[tokio::test]
    async fn client_gets_page_children_and_optional_folders() {
        let (base_url, requests) = queued_mock_server(vec![
            (
                StatusCode::OK,
                json!({
                    "results": [{
                        "id": "201",
                        "title": "Child page",
                        "type": "page",
                        "body": {"storage": {"value": "<p>Child</p>"}}
                    }]
                }),
            ),
            (
                StatusCode::OK,
                json!({
                    "results": [{
                        "id": "301",
                        "title": "Folder",
                        "type": "folder"
                    }]
                }),
            ),
        ])
        .await;

        let children = client(base_url)
            .get_page_children("123", Some(0), Some(2), &["version", "body.storage"], true)
            .await
            .unwrap();

        assert_eq!(children.len(), 2);
        assert_eq!(children[0].content_type.as_deref(), Some("page"));
        assert_eq!(children[1].content_type.as_deref(), Some("folder"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0]
                .path
                .starts_with("/rest/api/content/123/child/page?")
        );
        assert!(
            requests[1]
                .path
                .starts_with("/rest/api/content/123/child/folder?")
        );
        assert_eq!(
            query_value(&requests[0].path, "limit").as_deref(),
            Some("2")
        );
    }

    #[tokio::test]
    async fn client_gets_space_pages_with_ancestors_expand() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [{
                    "id": "100",
                    "title": "Home",
                    "ancestors": [],
                    "extensions": {"position": 0}
                }],
                "_links": {}
            }),
            StatusCode::OK,
        )
        .await;

        let response = client(base_url)
            .get_space_pages("ENG", Some(0), Some(1), &["ancestors"])
            .await
            .unwrap();

        assert_eq!(response.results.len(), 1);
        let requests = requests.lock().await;
        assert!(requests[0].path.starts_with("/rest/api/content?"));
        assert_eq!(
            query_value(&requests[0].path, "spaceKey").as_deref(),
            Some("ENG")
        );
        assert_eq!(
            query_value(&requests[0].path, "type").as_deref(),
            Some("page")
        );
        assert_eq!(
            query_value(&requests[0].path, "expand").as_deref(),
            Some("ancestors")
        );
    }

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
    async fn client_gets_labels_for_content_id() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [
                    {"id": "label-1", "name": "draft", "prefix": "global", "label": "draft", "type": "label"},
                    {"id": "label-2", "name": "team", "prefix": "my", "label": "team", "type": "label"}
                ],
                "start": 0,
                "limit": 200,
                "size": 2
            }),
            StatusCode::OK,
        )
        .await;

        let response = client(base_url).get_labels("123").await.unwrap();

        assert_eq!(response.results.len(), 2);
        assert_eq!(response.results[0].name.as_deref(), Some("draft"));
        assert_eq!(response.results[1].prefix.as_deref(), Some("my"));
        let requests = requests.lock().await;
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[0].path, "/rest/api/content/123/label");
    }

    #[tokio::test]
    async fn client_adds_label_and_refreshes_label_list() {
        let (base_url, requests) = queued_mock_server(vec![
            (StatusCode::OK, json!({})),
            (
                StatusCode::OK,
                json!({
                    "results": [
                        {"id": "label-1", "name": "draft", "prefix": "global", "label": "draft", "type": "label"}
                    ],
                    "start": 0,
                    "limit": 200,
                    "size": 1
                }),
            ),
        ])
        .await;

        let response = client(base_url).add_label("123", "draft").await.unwrap();

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].name.as_deref(), Some("draft"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/api/content/123/label");
        assert_eq!(requests[0].body[0]["prefix"], json!("global"));
        assert_eq!(requests[0].body[0]["name"], json!("draft"));
        assert_eq!(requests[1].method, Method::GET);
        assert_eq!(requests[1].path, "/rest/api/content/123/label");
    }

    #[tokio::test]
    async fn client_gets_cloud_page_views_with_optional_title() {
        let (base_url, requests) = queued_mock_server(vec![
            (
                StatusCode::OK,
                json!({
                    "id": "123",
                    "title": "Roadmap"
                }),
            ),
            (
                StatusCode::OK,
                json!({
                    "count": 42,
                    "lastSeen": "2026-06-04T12:00:00Z"
                }),
            ),
        ])
        .await;

        let views = cloud_client(base_url)
            .get_page_views("123", true)
            .await
            .unwrap();

        assert_eq!(views.page_id.as_deref(), Some("123"));
        assert_eq!(views.title.as_deref(), Some("Roadmap"));
        assert_eq!(views.count, Some(42));
        assert_eq!(views.last_seen.as_deref(), Some("2026-06-04T12:00:00Z"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::GET);
        assert!(requests[0].path.starts_with("/rest/api/content/123?"));
        assert_eq!(
            query_value(&requests[0].path, "expand").as_deref(),
            Some("title")
        );
        assert_eq!(requests[1].path, "/rest/api/analytics/content/123/views");
    }

    #[tokio::test]
    async fn client_gets_cloud_page_views_without_title_lookup() {
        let (base_url, requests) = mock_server(
            json!({
                "count": 7,
                "lastSeen": "2026-06-04T12:00:00Z"
            }),
            StatusCode::OK,
        )
        .await;

        let views = cloud_client(base_url)
            .get_page_views("123", false)
            .await
            .unwrap();

        assert_eq!(views.page_id.as_deref(), Some("123"));
        assert_eq!(views.title, None);
        assert_eq!(views.count, Some(7));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/rest/api/analytics/content/123/views");
    }

    #[tokio::test]
    async fn client_rejects_page_views_for_server_before_http_request() {
        let (base_url, requests) = mock_server(json!({}), StatusCode::OK).await;
        let error = client(base_url)
            .get_page_views("123", true)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("only available for Confluence Cloud")
        );
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn client_gets_content_attachments_with_pagination() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [
                    {
                        "id": "att-1",
                        "type": "attachment",
                        "title": "file.png",
                        "status": "current",
                        "extensions": {"mediaType": "image/png", "fileSize": 42},
                        "_links": {"download": "/download/attachments/att-1/file.png"}
                    },
                    {
                        "id": "att-2",
                        "type": "attachment",
                        "title": "notes.txt",
                        "metadata": {"mediaType": "text/plain", "fileSize": 12}
                    }
                ],
                "start": 5,
                "limit": 2,
                "size": 2,
                "_links": {"next": "/rest/api/content/123/child/attachment?start=7"}
            }),
            StatusCode::OK,
        )
        .await;

        let response = client(base_url)
            .get_attachments("123", Some(5), Some(2), None, None)
            .await
            .unwrap();

        assert_eq!(response.results.len(), 2);
        assert_eq!(response.results[0].media_type(), Some("image/png"));
        assert_eq!(response.results[1].media_type(), Some("text/plain"));
        let requests = requests.lock().await;
        assert!(
            requests[0]
                .path
                .starts_with("/rest/api/content/123/child/attachment?")
        );
        assert_eq!(
            query_value(&requests[0].path, "start").as_deref(),
            Some("5")
        );
        assert_eq!(
            query_value(&requests[0].path, "limit").as_deref(),
            Some("2")
        );
        assert_eq!(
            query_value(&requests[0].path, "expand").as_deref(),
            Some("metadata,extensions,version")
        );
    }

    #[tokio::test]
    async fn client_filters_content_attachments_by_filename_and_media_type() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [
                    {
                        "id": "att-1",
                        "title": "file.png",
                        "extensions": {"mediaType": "image/png"}
                    },
                    {
                        "id": "att-2",
                        "title": "notes.txt",
                        "metadata": {"mediaType": "text/plain"}
                    }
                ],
                "start": 0,
                "limit": 50,
                "size": 2
            }),
            StatusCode::OK,
        )
        .await;

        let response = client(base_url)
            .get_attachments("123", None, None, Some("notes.txt"), Some("text/plain"))
            .await
            .unwrap();

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].id.as_deref(), Some("att-2"));
        assert_eq!(response.size, Some(1));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(query_value(&requests[0].path, "filename").is_none());
        assert!(query_value(&requests[0].path, "media-type").is_none());
    }

    #[tokio::test]
    async fn client_rejects_invalid_attachment_limit_before_http() {
        let (base_url, requests) = mock_server(json!({}), StatusCode::OK).await;
        let error = client(base_url)
            .get_attachments("123", None, Some(101), None, None)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("limit must be less than or equal to 100")
        );
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn client_searches_cloud_users_with_cql_endpoint() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [{
                    "title": "Ada Lovelace",
                    "entityType": "user",
                    "score": 0.9,
                    "user": {
                        "accountId": "abc",
                        "displayName": "Ada Lovelace",
                        "email": "ada@example.com",
                        "accountStatus": "active"
                    }
                }],
                "start": 0,
                "limit": 5,
                "totalSize": 1,
                "cqlQuery": "user.fullname ~ \"Ada\""
            }),
            StatusCode::OK,
        )
        .await;

        let response = cloud_client(base_url)
            .search_user("user.fullname ~ \"Ada\"", Some(5), None)
            .await
            .unwrap();

        assert_eq!(response.results.len(), 1);
        assert_eq!(
            response.results[0]
                .user
                .as_ref()
                .and_then(|user| user.display_name.as_deref()),
            Some("Ada Lovelace")
        );
        let requests = requests.lock().await;
        assert_eq!(requests[0].method, Method::GET);
        assert!(requests[0].path.starts_with("/rest/api/search/user?"));
        assert_eq!(
            query_value(&requests[0].path, "cql").as_deref(),
            Some("user.fullname ~ \"Ada\"")
        );
        assert_eq!(
            query_value(&requests[0].path, "limit").as_deref(),
            Some("5")
        );
    }

    #[tokio::test]
    async fn client_searches_server_users_through_group_members() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [
                    {"username": "ada", "displayName": "Ada Lovelace", "email": "ada@example.com"},
                    {"username": "grace", "displayName": "Grace Hopper", "email": "grace@example.com"}
                ],
                "start": 0,
                "limit": 200,
                "size": 2,
                "_links": {}
            }),
            StatusCode::OK,
        )
        .await;

        let response = client(base_url)
            .search_user(
                "user.fullname ~ \"Ada\"",
                Some(10),
                Some("confluence users"),
            )
            .await
            .unwrap();

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].title.as_deref(), Some("Ada Lovelace"));
        assert_eq!(
            response.results[0].to_simplified_value()["user"]["active"],
            json!(true)
        );
        let requests = requests.lock().await;
        assert_eq!(requests[0].method, Method::GET);
        assert!(
            requests[0]
                .path
                .starts_with("/rest/api/group/confluence%20users/member?")
        );
        assert_eq!(
            query_value(&requests[0].path, "start").as_deref(),
            Some("0")
        );
        assert_eq!(
            query_value(&requests[0].path, "limit").as_deref(),
            Some("200")
        );
    }

    #[tokio::test]
    async fn client_search_user_rejects_invalid_limit_before_http_request() {
        let (base_url, requests) = mock_server(json!({}), StatusCode::OK).await;

        let error = client(base_url)
            .search_user("Ada", Some(51), None)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("limit must be less than or equal to 50")
        );
        assert_eq!(requests.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn client_search_user_preserves_auth_errors() {
        let (base_url, _requests) = mock_server(
            json!({"errorMessages": ["auth failed"]}),
            StatusCode::UNAUTHORIZED,
        )
        .await;

        let error = cloud_client(base_url)
            .search_user("user.fullname ~ \"Ada\"", Some(10), None)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AtlassianError::HttpStatus { status: 401, .. }
        ));
    }

    #[tokio::test]
    async fn client_parses_search_response() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [{
                    "id": "123",
                    "title": "Roadmap",
                    "content": {"id": "123", "title": "Roadmap"}
                }],
                "start": 0,
                "limit": 10,
                "size": 1
            }),
            StatusCode::OK,
        )
        .await;
        let response = client(base_url)
            .search_cql("space = ENG", Some(0), Some(10))
            .await
            .unwrap();

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].title.as_deref(), Some("Roadmap"));
        assert!(
            requests.lock().await[0]
                .path
                .starts_with("/rest/api/content/search?")
        );
    }

    #[tokio::test]
    async fn search_content_converts_simple_query_to_site_search_and_applies_spaces_filter() {
        let (base_url, requests) = mock_server(
            json!({
                "results": [{
                    "id": "123",
                    "title": "Roadmap",
                    "content": {"id": "123", "title": "Roadmap"}
                }],
                "start": 0,
                "limit": 10,
                "size": 1
            }),
            StatusCode::OK,
        )
        .await;

        let response = client(base_url)
            .search_content("project docs", Some(10), Some("ENG, ~me"))
            .await
            .unwrap();

        assert_eq!(response.results.len(), 1);
        let requests = requests.lock().await;
        assert_eq!(
            query_value(&requests[0].path, "cql").as_deref(),
            Some(r#"(siteSearch ~ "project docs") AND (space = ENG OR space = "~me")"#)
        );
        assert_eq!(
            query_value(&requests[0].path, "limit").as_deref(),
            Some("10")
        );
    }

    #[tokio::test]
    async fn search_content_falls_back_to_text_search_when_site_search_is_rejected() {
        let (base_url, requests) = queued_mock_server(vec![
            (
                StatusCode::BAD_REQUEST,
                json!({"errorMessages": ["siteSearch unsupported"]}),
            ),
            (
                StatusCode::OK,
                json!({
                    "results": [{"id": "123", "title": "Roadmap"}],
                    "start": 0,
                    "limit": 10,
                    "size": 1
                }),
            ),
        ])
        .await;

        let response = client(base_url)
            .search_content("project docs", Some(10), None)
            .await
            .unwrap();

        assert_eq!(response.results.len(), 1);
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(
            query_value(&requests[0].path, "cql").as_deref(),
            Some(r#"siteSearch ~ "project docs""#)
        );
        assert_eq!(
            query_value(&requests[1].path, "cql").as_deref(),
            Some(r#"text ~ "project docs""#)
        );
    }

    #[tokio::test]
    async fn search_content_does_not_fallback_on_auth_error() {
        let (base_url, requests) = queued_mock_server(vec![
            (
                StatusCode::UNAUTHORIZED,
                json!({"errorMessages": ["auth failed"]}),
            ),
            (
                StatusCode::OK,
                json!({"results": [{"id": "should-not-be-read"}]}),
            ),
        ])
        .await;

        let error = client(base_url)
            .search_content("project docs", Some(10), None)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AtlassianError::HttpStatus { status: 401, .. }
        ));
        assert_eq!(requests.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn search_content_uses_config_space_filter_and_explicit_empty_disables_it() {
        let (base_url, requests) = mock_server(json!({"results": []}), StatusCode::OK).await;
        let client = client_with_spaces_filter(
            base_url,
            BTreeSet::from(["ENG".to_string(), "~personal".to_string()]),
        );

        client
            .search_content("type=page", Some(10), None)
            .await
            .unwrap();
        client
            .search_content("type=page", Some(10), Some(""))
            .await
            .unwrap();

        let requests = requests.lock().await;
        assert_eq!(
            query_value(&requests[0].path, "cql").as_deref(),
            Some(r#"(type=page) AND (space = ENG OR space = "~personal")"#)
        );
        assert_eq!(
            query_value(&requests[1].path, "cql").as_deref(),
            Some("type=page")
        );
    }

    #[tokio::test]
    async fn search_content_rejects_invalid_limit_before_request() {
        let (base_url, requests) = mock_server(json!({"results": []}), StatusCode::OK).await;

        let error = client(base_url)
            .search_content("docs", Some(51), None)
            .await
            .unwrap_err();

        assert!(matches!(error, AtlassianError::InvalidInput { .. }));
        assert_eq!(requests.lock().await.len(), 0);
    }
}
