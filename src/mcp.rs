use std::{path::Path, sync::Arc, time::Instant};

use crate::{
    atlassian::error::AtlassianError,
    confluence::{
        client::ConfluenceClient,
        config::ConfluenceDeployment,
        formatting::{ConfluenceContentFormat, content_to_storage},
        models::{ConfluenceAttachment, ConfluencePage},
        tools as confluence_tools,
    },
    context::AppContext,
    jira::{
        client::{
            AttachmentFetchOptions, DEFAULT_ATTACHMENT_MAX_BYTES, FieldOptionsRequest,
            GetIssueRequest, JiraClient, SearchRequest,
        },
        config::JiraDeployment,
        formatting::{
            base64_encode, comment_body_for_deployment, merge_optional_objects,
            parse_optional_object, parse_optional_string_list, parse_required_object,
            parse_required_object_list, parse_required_string_list, redact_url_query,
        },
        tools::{
            JiraAddCommentArgs, JiraAddIssuesToSprintArgs, JiraAddWatcherArgs, JiraAddWorklogArgs,
            JiraBatchCreateIssuesArgs, JiraBatchCreateVersionsArgs, JiraBatchGetChangelogsArgs,
            JiraCreateIssueArgs, JiraCreateIssueLinkArgs, JiraCreateRemoteIssueLinkArgs,
            JiraCreateSprintArgs, JiraCreateVersionArgs, JiraDeleteIssueArgs,
            JiraDownloadAttachmentsArgs, JiraEditCommentArgs, JiraGetAgileBoardsArgs,
            JiraGetAllProjectsArgs, JiraGetBoardIssuesArgs, JiraGetFieldOptionsArgs,
            JiraGetIssueArgs, JiraGetIssueDatesArgs, JiraGetIssueDevelopmentInfoArgs,
            JiraGetIssueImagesArgs, JiraGetIssueProformaFormsArgs, JiraGetIssueSlaArgs,
            JiraGetIssueWatchersArgs, JiraGetIssuesDevelopmentInfoArgs, JiraGetLinkTypesArgs,
            JiraGetProformaFormDetailsArgs, JiraGetProjectComponentsArgs, JiraGetProjectIssuesArgs,
            JiraGetProjectVersionsArgs, JiraGetQueueIssuesArgs, JiraGetServiceDeskForProjectArgs,
            JiraGetServiceDeskQueuesArgs, JiraGetSprintIssuesArgs, JiraGetSprintsFromBoardArgs,
            JiraGetTransitionsArgs, JiraGetUserProfileArgs, JiraGetWorklogArgs, JiraLinkToEpicArgs,
            JiraRemoveIssueLinkArgs, JiraRemoveWatcherArgs, JiraSearchArgs, JiraSearchFieldsArgs,
            JiraTransitionIssueArgs, JiraUpdateIssueArgs, JiraUpdateProformaFormAnswersArgs,
            JiraUpdateSprintArgs,
        },
    },
    tool_registry,
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::tool::ToolCallContext,
    handler::server::wrapper::Parameters,
    model::{
        CallToolRequestParams, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde_json::{Map, Value, json};

pub const SERVER_NAME: &str = "mcp-atlassian-rs";

const MIGRATION_STATUS: &str = "mcp-atlassian-rs Stage 2 Jira core migration is complete. \
The Stage 1 MCP runtime/control plane and Stage 2 Jira config/auth/client/models/tool handlers are implemented. \
Jira core tools are available when Jira configuration and authentication are complete. \
Mock REST tests, MCP smoke checks, README, and the migration ledger are up to date.";

const CONFLUENCE_PAGE_EXPAND: &[&str] = &[
    "body.storage",
    "version",
    "space",
    "ancestors",
    "metadata.labels",
    "history",
    "children.attachment",
];
const CONFLUENCE_CHILDREN_DEFAULT_LIMIT: u64 = 25;
const CONFLUENCE_CHILDREN_MAX_LIMIT: u64 = 50;
const CONFLUENCE_TREE_DEFAULT_LIMIT: u64 = 500;
const CONFLUENCE_TREE_MAX_LIMIT: u64 = 1_000;
const CONFLUENCE_TREE_PAGE_SIZE: u64 = 200;

#[derive(Clone)]
pub struct AtlassianMcpServer {
    context: Arc<AppContext>,
    tool_router: ToolRouter<Self>,
}

impl AtlassianMcpServer {
    pub fn new(context: Arc<AppContext>) -> Self {
        Self {
            context,
            tool_router: Self::tool_router(),
        }
    }

    fn current_tools_result(&self) -> ListToolsResult {
        ListToolsResult {
            tools: sanitize_tools_for_clients(
                self.filtered_tools_from(self.tool_router.list_all()),
            ),
            ..Default::default()
        }
    }

    fn filtered_tools_from<I>(&self, tools: I) -> Vec<Tool>
    where
        I: IntoIterator<Item = Tool>,
    {
        tool_registry::visible_tools(tools, &self.context)
    }

    fn guard_registered_tool_call(&self, name: &str) -> Result<(), ErrorData> {
        if !self.tool_router.has_route(name) {
            return Err(ErrorData::invalid_params("tool not available", None));
        }

        tool_registry::guard_tool_call(name, &self.context)
    }

    fn jira_client(&self) -> Result<JiraClient, ErrorData> {
        let Some(config) = self.context.jira_config() else {
            return Err(ErrorData::invalid_params("Jira is not configured", None));
        };

        JiraClient::new(config.clone()).map_err(jira_error)
    }

    #[allow(dead_code)]
    fn confluence_client(&self) -> Result<ConfluenceClient, ErrorData> {
        let Some(config) = self.context.confluence_config() else {
            return Err(ErrorData::invalid_params(
                "Confluence is not configured",
                None,
            ));
        };

        ConfluenceClient::new(config.clone()).map_err(jira_error)
    }

    #[cfg(test)]
    fn guard_tool_call_with_metadata<F>(
        &self,
        name: &str,
        route_exists: bool,
        metadata_for: F,
    ) -> Result<(), ErrorData>
    where
        F: Fn(&str) -> Option<tool_registry::ToolMetadata>,
    {
        if !route_exists {
            return Err(ErrorData::invalid_params("tool not available", None));
        }

        tool_registry::guard_tool_call_with_metadata(name, &self.context, metadata_for)
    }

    #[cfg(test)]
    fn filtered_tools_from_with_metadata<I, F>(&self, tools: I, metadata_for: F) -> Vec<Tool>
    where
        I: IntoIterator<Item = Tool>,
        F: Fn(&str) -> Option<tool_registry::ToolMetadata>,
    {
        tool_registry::visible_tools_with_metadata(tools, &self.context, metadata_for)
    }
}

impl Default for AtlassianMcpServer {
    fn default() -> Self {
        Self::new(Arc::new(AppContext::default()))
    }
}

#[tool_router(router = tool_router)]
impl AtlassianMcpServer {
    #[tool(description = "Report the current Rust migration status for MCP Atlassian")]
    fn migration_status(&self) -> String {
        MIGRATION_STATUS.to_string()
    }

    #[tool(description = "Search Confluence content using simple terms or CQL")]
    async fn confluence_search(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceSearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = required_non_empty_arg(args.query, "query")?;
        let limit = optional_confluence_search_limit_arg(args.limit)?;
        let value = self
            .confluence_client()?
            .search_content(&query, limit, args.spaces_filter.as_deref())
            .await
            .map_err(jira_error)?
            .to_simplified_value();

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get a Confluence page by ID or title and space key")]
    async fn confluence_get_page(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetPageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let include_metadata = args.include_metadata.unwrap_or(true);
        let convert_to_markdown = args.convert_to_markdown.unwrap_or(true);
        let client = self.confluence_client()?;

        if let Some(page_id) = optional_non_empty_arg(args.page_id) {
            let page = match client
                .get_page_by_id(&page_id, CONFLUENCE_PAGE_EXPAND)
                .await
            {
                Ok(page) => page,
                Err(AtlassianError::HttpStatus { status: 404, .. }) => {
                    return Ok(CallToolResult::structured(json!({
                        "error": format!("Failed to retrieve page by ID '{page_id}': page not found")
                    })));
                }
                Err(error) => return Err(jira_error(error)),
            };

            return Ok(CallToolResult::structured(confluence_page_tool_value(
                &page,
                include_metadata,
                convert_to_markdown,
            )));
        }

        let title = optional_non_empty_arg(args.title);
        let space_key = optional_non_empty_arg(args.space_key);
        let (Some(title), Some(space_key)) = (title, space_key) else {
            return Err(jira_error(AtlassianError::invalid_input(
                "Either page_id OR both title and space_key must be provided",
            )));
        };

        let Some(page) = client
            .get_page_by_title(&space_key, &title, CONFLUENCE_PAGE_EXPAND)
            .await
            .map_err(jira_error)?
        else {
            return Ok(CallToolResult::structured(json!({
                "error": format!("Page with title '{title}' not found in space '{space_key}'.")
            })));
        };

        Ok(CallToolResult::structured(confluence_page_tool_value(
            &page,
            include_metadata,
            convert_to_markdown,
        )))
    }

    #[tool(description = "List child pages and folders for a Confluence page")]
    async fn confluence_get_page_children(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetPageChildrenArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let parent_id = required_non_empty_arg(args.parent_id, "parent_id")?;
        let limit = optional_u64_range_arg(
            args.limit,
            CONFLUENCE_CHILDREN_DEFAULT_LIMIT,
            CONFLUENCE_CHILDREN_MAX_LIMIT,
            "limit",
        )?;
        let start = args.start.unwrap_or(0);
        let include_content = args.include_content.unwrap_or(false);
        let include_folders = args.include_folders.unwrap_or(true);
        let convert_to_markdown = args.convert_to_markdown.unwrap_or(true);
        let expand = confluence_expand_list(args.expand, include_content);
        let expand_refs = expand.iter().map(String::as_str).collect::<Vec<_>>();
        let children = self
            .confluence_client()?
            .get_page_children(
                &parent_id,
                Some(start),
                Some(limit),
                &expand_refs,
                include_folders,
            )
            .await
            .map_err(jira_error)?;
        let results = children
            .iter()
            .map(|page| confluence_child_page_value(page, include_content, convert_to_markdown))
            .collect::<Vec<_>>();

        Ok(CallToolResult::structured(json!({
            "parent_id": parent_id,
            "count": results.len(),
            "limit_requested": limit,
            "start_requested": start,
            "results": results,
        })))
    }

    #[tool(description = "Get a flat page hierarchy for a Confluence space")]
    async fn confluence_get_space_page_tree(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetSpacePageTreeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let space_key = required_non_empty_arg(args.space_key, "space_key")?;
        let limit = optional_u64_range_arg(
            args.limit,
            CONFLUENCE_TREE_DEFAULT_LIMIT,
            CONFLUENCE_TREE_MAX_LIMIT,
            "limit",
        )?;
        let client = self.confluence_client()?;
        let mut pages = Vec::new();
        let mut start = 0;
        let mut next_link_exists = false;

        while pages.len() < limit as usize {
            let fetch_limit = CONFLUENCE_TREE_PAGE_SIZE.min(limit - pages.len() as u64);
            let response = client
                .get_space_pages(&space_key, Some(start), Some(fetch_limit), &["ancestors"])
                .await
                .map_err(jira_error)?;
            let batch_len = response.results.len() as u64;
            next_link_exists = response.links.get("next").and_then(Value::as_str).is_some();
            pages.extend(response.results);

            if batch_len == 0 || !next_link_exists {
                break;
            }
            start += batch_len;
        }

        let has_more = pages.len() >= limit as usize && next_link_exists;
        let mut tree_pages = pages
            .iter()
            .map(confluence_tree_page_sort_value)
            .collect::<Vec<_>>();
        tree_pages.sort_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then(left.position_sort.cmp(&right.position_sort))
                .then(left.title.cmp(&right.title))
        });
        let result_pages = tree_pages
            .into_iter()
            .map(|page| page.value)
            .collect::<Vec<_>>();
        let mut result = json!({
            "space_key": space_key,
            "total_pages": result_pages.len(),
            "has_more": has_more,
            "pages": result_pages,
        });
        if has_more {
            result["next_start"] = Value::from(start);
        }

        Ok(CallToolResult::structured(result))
    }

    #[tool(description = "Create a Confluence page")]
    async fn confluence_create_page(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceCreatePageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let space_key = required_non_empty_arg(args.space_key, "space_key")?;
        let title = required_non_empty_arg(args.title, "title")?;
        let content = required_non_empty_arg(args.content, "content")?;
        let format = parse_confluence_write_content_format(args.content_format.as_deref())?;
        let storage_body = content_to_storage(&content, format);
        let parent_id = optional_non_empty_arg(args.parent_id);
        let client = self.confluence_client()?;
        let page = client
            .create_page(&space_key, &title, &storage_body, parent_id.as_deref())
            .await
            .map_err(jira_error)?;
        if let Some(page_id) = page.id.as_deref() {
            client
                .set_page_emoji_best_effort(page_id, args.emoji.as_deref())
                .await;
        }

        Ok(CallToolResult::structured(json!({
            "message": "Page created successfully",
            "page": confluence_write_page_value(&page, args.include_content.unwrap_or(false)),
        })))
    }

    #[tool(description = "Update a Confluence page")]
    async fn confluence_update_page(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceUpdatePageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let page_id = required_non_empty_arg(args.page_id, "page_id")?;
        let title = required_non_empty_arg(args.title, "title")?;
        let content = required_non_empty_arg(args.content, "content")?;
        let format = parse_confluence_write_content_format(args.content_format.as_deref())?;
        let storage_body = content_to_storage(&content, format);
        let parent_id = optional_non_empty_arg(args.parent_id);
        let client = self.confluence_client()?;
        let page = client
            .update_page(
                &page_id,
                &title,
                &storage_body,
                parent_id.as_deref(),
                args.is_minor_edit.unwrap_or(false),
                args.version_comment.as_deref(),
            )
            .await
            .map_err(jira_error)?;
        if let Some(page_id) = page.id.as_deref() {
            client
                .set_page_emoji_best_effort(page_id, args.emoji.as_deref())
                .await;
        }

        Ok(CallToolResult::structured(json!({
            "message": "Page updated successfully",
            "page": confluence_write_page_value(&page, args.include_content.unwrap_or(false)),
        })))
    }

    #[tool(description = "Delete a Confluence page")]
    async fn confluence_delete_page(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceDeletePageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let page_id = required_non_empty_arg(args.page_id, "page_id")?;
        match self.confluence_client()?.delete_page(&page_id).await {
            Ok(_) => Ok(CallToolResult::structured(json!({
                "success": true,
                "message": format!("Page {page_id} deleted successfully"),
            }))),
            Err(error) => Ok(CallToolResult::structured(json!({
                "success": false,
                "message": format!("Error deleting page {page_id}"),
                "error": error.to_string(),
            }))),
        }
    }

    #[tool(description = "Move a Confluence page to a new parent or space")]
    async fn confluence_move_page(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceMovePageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let page_id = required_non_empty_arg(args.page_id, "page_id")?;
        let target_parent_id = optional_non_empty_arg(args.target_parent_id);
        let page = self
            .confluence_client()?
            .move_page(
                &page_id,
                target_parent_id.as_deref(),
                args.target_space_key.as_deref(),
                args.position.as_deref(),
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(json!({
            "message": "Page moved successfully",
            "page": confluence_write_page_value(&page, true),
        })))
    }

    #[tool(description = "List comments for a Confluence page")]
    async fn confluence_get_comments(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetCommentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let page_id = required_non_empty_arg(args.page_id, "page_id")?;
        let comments = self
            .confluence_client()?
            .get_page_comments(&page_id)
            .await
            .map_err(jira_error)?;
        let values = comments
            .results
            .iter()
            .map(|comment| comment.to_simplified_value())
            .collect::<Vec<_>>();

        Ok(CallToolResult::structured(json!({
            "page_id": page_id,
            "count": values.len(),
            "comments": values,
            "start": comments.start,
            "limit": comments.limit,
            "size": comments.size,
            "links": comments.links,
        })))
    }

    #[tool(description = "Add a comment to a Confluence page")]
    async fn confluence_add_comment(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceAddCommentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let page_id = required_non_empty_arg(args.page_id, "page_id")?;
        let body = required_non_empty_arg(args.body, "body")?;
        let storage_body = content_to_storage(&body, ConfluenceContentFormat::Markdown);

        match self
            .confluence_client()?
            .add_comment(&page_id, &storage_body)
            .await
        {
            Ok(comment) => Ok(CallToolResult::structured(json!({
                "success": true,
                "message": "Comment added successfully",
                "comment": comment.to_simplified_value(),
            }))),
            Err(error) => Ok(CallToolResult::structured(json!({
                "success": false,
                "message": format!("Error adding comment to page {page_id}"),
                "error": error.to_string(),
            }))),
        }
    }

    #[tool(description = "Reply to a Confluence comment thread")]
    async fn confluence_reply_to_comment(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceReplyToCommentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let comment_id = required_non_empty_arg(args.comment_id, "comment_id")?;
        let body = required_non_empty_arg(args.body, "body")?;
        let storage_body = content_to_storage(&body, ConfluenceContentFormat::Markdown);

        match self
            .confluence_client()?
            .reply_to_comment(&comment_id, &storage_body)
            .await
        {
            Ok(comment) => Ok(CallToolResult::structured(json!({
                "success": true,
                "message": "Reply added successfully",
                "comment": comment.to_simplified_value(),
            }))),
            Err(error) => Ok(CallToolResult::structured(json!({
                "success": false,
                "message": format!("Error replying to comment {comment_id}"),
                "error": error.to_string(),
            }))),
        }
    }

    #[tool(description = "List labels for Confluence content")]
    async fn confluence_get_labels(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetLabelsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let content_id = required_non_empty_arg(args.page_id, "page_id")?;
        let labels = self
            .confluence_client()?
            .get_labels(&content_id)
            .await
            .map_err(jira_error)?;
        let values = labels
            .results
            .iter()
            .map(|label| label.to_simplified_value())
            .collect::<Vec<_>>();

        Ok(CallToolResult::structured(json!({
            "content_id": content_id,
            "count": values.len(),
            "labels": values,
            "start": labels.start,
            "limit": labels.limit,
            "size": labels.size,
            "links": labels.links,
        })))
    }

    #[tool(description = "Add a label to Confluence content")]
    async fn confluence_add_label(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceAddLabelArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let content_id = required_non_empty_arg(args.page_id, "page_id")?;
        let name = required_non_empty_arg(args.name, "name")?;
        let labels = self
            .confluence_client()?
            .add_label(&content_id, &name)
            .await
            .map_err(jira_error)?;
        let values = labels
            .results
            .iter()
            .map(|label| label.to_simplified_value())
            .collect::<Vec<_>>();

        Ok(CallToolResult::structured(json!({
            "message": "Label added successfully",
            "content_id": content_id,
            "count": values.len(),
            "labels": values,
            "start": labels.start,
            "limit": labels.limit,
            "size": labels.size,
            "links": labels.links,
        })))
    }

    #[tool(description = "Search Confluence users")]
    async fn confluence_search_user(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceSearchUserArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = required_non_empty_arg(args.query, "query")?;
        let limit = confluence_user_search_limit(args.limit)?;
        let group_name = optional_non_empty_arg(args.group_name)
            .unwrap_or_else(|| "confluence-users".to_string());
        let cql = normalize_confluence_user_search_query(&query);

        match self
            .confluence_client()?
            .search_user(&cql, Some(limit), Some(&group_name))
            .await
        {
            Ok(response) => {
                let results = response.to_simplified_value()["results"].clone();
                let cql_query = response.cql_query.clone().unwrap_or_else(|| cql.clone());
                Ok(CallToolResult::structured(json!({
                    "group_name": group_name,
                    "count": response.results.len(),
                    "results": results,
                    "start": response.start,
                    "limit": response.limit,
                    "size": response.size,
                    "total_size": response.total_size,
                    "cql_query": cql_query,
                    "search_duration": response.search_duration,
                    "links": response.links,
                })))
            }
            Err(AtlassianError::HttpStatus {
                status, message, ..
            }) if matches!(status, 401 | 403) => Ok(CallToolResult::structured(json!({
                "success": false,
                "error": "Authentication failed. Please check your credentials.",
                "status": status,
                "details": message,
            }))),
            Err(error) => Err(jira_error(error)),
        }
    }

    #[tool(description = "Get a historical version of a Confluence page")]
    async fn confluence_get_page_history(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetPageHistoryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let page_id = required_non_empty_arg(args.page_id, "page_id")?;
        let version = confluence_positive_version_arg(args.version, "version")?;
        let convert_to_markdown = args.convert_to_markdown.unwrap_or(true);
        let page = self
            .confluence_client()?
            .get_page_history(&page_id, version, CONFLUENCE_PAGE_EXPAND)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(
            page.to_simplified_value(convert_to_markdown),
        ))
    }

    #[tool(description = "Get a diff between two Confluence page versions")]
    async fn confluence_get_page_diff(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetPageDiffArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let page_id = required_non_empty_arg(args.page_id, "page_id")?;
        let from_version = confluence_positive_version_arg(args.from_version, "from_version")?;
        let to_version = confluence_positive_version_arg(args.to_version, "to_version")?;
        if from_version > to_version {
            return Err(jira_error(AtlassianError::invalid_input(
                "from_version must be less than or equal to to_version",
            )));
        }
        let client = self.confluence_client()?;
        let from_page = client
            .get_page_history(&page_id, from_version, CONFLUENCE_PAGE_EXPAND)
            .await
            .map_err(jira_error)?;
        let to_page = if from_version == to_version {
            from_page.clone()
        } else {
            client
                .get_page_history(&page_id, to_version, CONFLUENCE_PAGE_EXPAND)
                .await
                .map_err(jira_error)?
        };
        let from_content = confluence_page_markdown_content(&from_page);
        let to_content = confluence_page_markdown_content(&to_page);
        let diff = confluence_unified_diff(&from_content, &to_content, from_version, to_version);

        Ok(CallToolResult::structured(json!({
            "page_id": page_id,
            "title": to_page.title,
            "from_version": from_version,
            "to_version": to_version,
            "diff": diff,
            "has_changes": from_content != to_content,
        })))
    }

    #[tool(description = "Get Confluence Cloud page view analytics")]
    async fn confluence_get_page_views(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetPageViewsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let page_id = required_non_empty_arg(args.page_id, "page_id")?;
        let include_title = args.include_title.unwrap_or(true);
        let client = self.confluence_client()?;
        if client.config().deployment != ConfluenceDeployment::Cloud {
            return Ok(CallToolResult::structured(json!({
                "success": false,
                "available": false,
                "page_id": page_id,
                "error": "Page view analytics is only available for Confluence Cloud. Server/Data Center instances do not support the Analytics API.",
            })));
        }

        match client.get_page_views(&page_id, include_title).await {
            Ok(views) => Ok(CallToolResult::structured(views.to_simplified_value())),
            Err(AtlassianError::HttpStatus {
                status, message, ..
            }) if matches!(status, 401 | 403) => Ok(CallToolResult::structured(json!({
                "success": false,
                "error": "Authentication failed. Please check your credentials.",
                "status": status,
                "details": message,
            }))),
            Err(error) => Err(jira_error(error)),
        }
    }

    #[tool(description = "Upload an attachment to Confluence content")]
    async fn confluence_upload_attachment(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceUploadAttachmentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let content_id = required_non_empty_arg(args.content_id, "content_id")?;
        let file_path = required_non_empty_arg(args.file_path, "file_path")?;
        let filename = confluence_file_path_display(&file_path);
        let comment = optional_non_empty_arg(args.comment);
        let minor_edit = args.minor_edit.unwrap_or(false);
        let client = self.confluence_client()?;

        match client
            .upload_attachment(&content_id, &file_path, comment.as_deref(), minor_edit)
            .await
        {
            Ok(attachment) => Ok(CallToolResult::structured(json!({
                "success": true,
                "content_id": content_id,
                "filename": filename,
                "minor_edit": minor_edit,
                "attachment": attachment.to_simplified_value(),
            }))),
            Err(error) => Ok(CallToolResult::structured(json!({
                "success": false,
                "content_id": content_id,
                "filename": filename,
                "minor_edit": minor_edit,
                "error": error.to_string(),
            }))),
        }
    }

    #[tool(description = "Upload multiple attachments to Confluence content")]
    async fn confluence_upload_attachments(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceUploadAttachmentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let content_id = required_non_empty_arg(args.content_id, "content_id")?;
        let file_paths = confluence_split_file_paths(&args.file_paths)?;
        let comment = optional_non_empty_arg(args.comment);
        let minor_edit = args.minor_edit.unwrap_or(false);
        let client = self.confluence_client()?;
        let mut uploaded = Vec::new();
        let mut failed = Vec::new();

        for (index, file_path) in file_paths.iter().enumerate() {
            let filename = confluence_file_path_display(file_path);
            match client
                .upload_attachment(&content_id, file_path, comment.as_deref(), minor_edit)
                .await
            {
                Ok(attachment) => uploaded.push(json!({
                    "index": index,
                    "filename": filename,
                    "attachment": attachment.to_simplified_value(),
                })),
                Err(error) => failed.push(json!({
                    "index": index,
                    "filename": filename,
                    "error": error.to_string(),
                })),
            }
        }

        Ok(CallToolResult::structured(json!({
            "success": failed.is_empty(),
            "partial_success": !uploaded.is_empty() && !failed.is_empty(),
            "content_id": content_id,
            "minor_edit": minor_edit,
            "summary": {
                "total": uploaded.len() + failed.len(),
                "uploaded": uploaded.len(),
                "failed": failed.len(),
            },
            "attachments": uploaded,
            "failed": failed,
        })))
    }

    #[tool(description = "List attachments for Confluence content")]
    async fn confluence_get_attachments(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetAttachmentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let content_id = required_non_empty_arg(args.content_id, "content_id")?;
        let start = args.start.unwrap_or(0);
        let limit = optional_u64_range_arg(
            args.limit,
            crate::confluence::client::DEFAULT_ATTACHMENT_LIST_LIMIT,
            crate::confluence::client::MAX_ATTACHMENT_LIST_LIMIT,
            "limit",
        )?;
        let filename = optional_non_empty_arg(args.filename);
        let media_type = optional_non_empty_arg(args.media_type);
        let response = self
            .confluence_client()?
            .get_attachments(
                &content_id,
                Some(start),
                Some(limit),
                filename.as_deref(),
                media_type.as_deref(),
            )
            .await
            .map_err(jira_error)?;
        let attachments = response
            .results
            .iter()
            .map(|attachment| attachment.to_simplified_value())
            .collect::<Vec<_>>();

        Ok(CallToolResult::structured(json!({
            "success": true,
            "content_id": content_id,
            "count": attachments.len(),
            "total": response.size.unwrap_or(attachments.len() as u64),
            "start": response.start.unwrap_or(start),
            "limit": response.limit.unwrap_or(limit),
            "filters": {
                "filename": filename,
                "media_type": media_type,
            },
            "attachments": attachments,
            "links": response.links,
        })))
    }

    #[tool(description = "Download one Confluence attachment with bounded content output")]
    async fn confluence_download_attachment(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceDownloadAttachmentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let attachment_id = required_non_empty_arg(args.attachment_id, "attachment_id")?;
        let client = self.confluence_client()?;
        let attachment = client
            .get_attachment_by_id(&attachment_id)
            .await
            .map_err(jira_error)?;

        match confluence_attachment_with_content_value(
            &client,
            &attachment,
            &attachment_id,
            crate::confluence::client::DEFAULT_ATTACHMENT_MAX_BYTES,
        )
        .await
        {
            Ok(attachment) => Ok(CallToolResult::structured(json!({
                "success": true,
                "attachment": attachment,
            }))),
            Err(error) => Ok(CallToolResult::structured(error)),
        }
    }

    #[tool(description = "Download all attachments for Confluence content with bounded output")]
    async fn confluence_download_content_attachments(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceDownloadContentAttachmentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let content_id = required_non_empty_arg(args.content_id, "content_id")?;
        let client = self.confluence_client()?;
        let response = client
            .get_attachments(
                &content_id,
                Some(0),
                Some(crate::confluence::client::MAX_ATTACHMENT_LIST_LIMIT),
                None,
                None,
            )
            .await
            .map_err(jira_error)?;
        let mut attachments = Vec::new();
        let mut failed = Vec::new();

        for attachment in &response.results {
            let attachment_id = confluence_attachment_id(attachment);
            match confluence_attachment_with_content_value(
                &client,
                attachment,
                &attachment_id,
                crate::confluence::client::DEFAULT_ATTACHMENT_MAX_BYTES,
            )
            .await
            {
                Ok(value) => attachments.push(value),
                Err(error) => failed.push(error),
            }
        }

        Ok(CallToolResult::structured(json!({
            "success": true,
            "summary": {
                "content_id": content_id,
                "total": response.results.len(),
                "downloaded": attachments.len(),
                "failed": failed.len(),
            },
            "attachments": attachments,
            "failed": failed,
        })))
    }

    #[tool(description = "Delete a Confluence attachment")]
    async fn confluence_delete_attachment(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceDeleteAttachmentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let attachment_id = required_non_empty_arg(args.attachment_id, "attachment_id")?;
        match self
            .confluence_client()?
            .delete_attachment(&attachment_id)
            .await
        {
            Ok(value) => Ok(CallToolResult::structured(json!({
                "success": true,
                "attachment_id": attachment_id,
                "result": value,
            }))),
            Err(error) => Ok(CallToolResult::structured(json!({
                "success": false,
                "attachment_id": attachment_id,
                "error": error.to_string(),
            }))),
        }
    }

    #[tool(description = "Get image attachments for Confluence content")]
    async fn confluence_get_page_images(
        &self,
        Parameters(args): Parameters<confluence_tools::ConfluenceGetPageImagesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let content_id = required_non_empty_arg(args.content_id, "content_id")?;
        let client = self.confluence_client()?;
        let response = client
            .get_attachments(
                &content_id,
                Some(0),
                Some(crate::confluence::client::MAX_ATTACHMENT_LIST_LIMIT),
                None,
                None,
            )
            .await
            .map_err(jira_error)?;
        let mut images = Vec::new();
        let mut failed = Vec::new();
        let mut skipped_non_images = 0usize;

        for attachment in &response.results {
            let filename =
                confluence_attachment_filename(attachment, &confluence_attachment_id(attachment));
            let (is_image, resolved_mime_type) =
                confluence_is_image_attachment(attachment.media_type(), &filename);
            if !is_image {
                skipped_non_images += 1;
                continue;
            }

            let attachment_id = confluence_attachment_id(attachment);
            match confluence_attachment_with_content_value(
                &client,
                attachment,
                &attachment_id,
                crate::confluence::client::DEFAULT_ATTACHMENT_MAX_BYTES,
            )
            .await
            {
                Ok(mut value) => {
                    value["is_image"] = Value::Bool(true);
                    value["resolved_mime_type"] = Value::String(resolved_mime_type);
                    images.push(value);
                }
                Err(error) => failed.push(error),
            }
        }

        Ok(CallToolResult::structured(json!({
            "success": true,
            "content_id": content_id,
            "images_only": true,
            "count": images.len(),
            "skipped_non_images": skipped_non_images,
            "images": images,
            "failed": failed,
        })))
    }

    #[tool(description = "Get a Jira issue by key")]
    async fn jira_get_issue(
        &self,
        Parameters(args): Parameters<JiraGetIssueArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let fields = parse_optional_string_list_arg(args.fields, "fields")?;
        let expand = parse_optional_string_list_arg(args.expand, "expand")?;
        let properties = parse_optional_string_list_arg(args.properties, "properties")?;
        let value = self
            .jira_client()?
            .get_issue(GetIssueRequest {
                issue_key: args.issue_key,
                fields,
                expand,
                comment_limit: args.comment_limit,
                properties,
                update_history: args.update_history,
            })
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Search Jira issues with JQL")]
    async fn jira_search(
        &self,
        Parameters(args): Parameters<JiraSearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let fields = parse_optional_string_list_arg(args.fields, "fields")?;
        let expand = parse_optional_string_list_arg(args.expand, "expand")?;
        let projects_filter =
            parse_optional_string_list_arg(args.projects_filter, "projects_filter")?;
        let value = self
            .jira_client()?
            .search(SearchRequest {
                jql: args.jql,
                fields,
                limit: args.limit,
                start_at: args.start_at,
                projects_filter,
                expand,
                page_token: args.page_token,
            })
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "List Jira issues for a project")]
    async fn jira_get_project_issues(
        &self,
        Parameters(args): Parameters<JiraGetProjectIssuesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_project_issues(args.project_key, args.limit, args.start_at)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Search Jira fields by keyword")]
    async fn jira_search_fields(
        &self,
        Parameters(args): Parameters<JiraSearchFieldsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .search_fields(args.keyword, args.limit)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get options for a Jira field")]
    async fn jira_get_field_options(
        &self,
        Parameters(args): Parameters<JiraGetFieldOptionsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_field_options(FieldOptionsRequest {
                field_id: args.field_id,
                context_id: args.context_id,
                project_key: args.project_key,
                issue_type: args.issue_type,
                contains: args.contains,
                return_limit: args.return_limit,
                values_only: args.values_only.unwrap_or(false),
            })
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Add a comment to a Jira issue")]
    async fn jira_add_comment(
        &self,
        Parameters(args): Parameters<JiraAddCommentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let visibility = parse_optional_object_arg(args.visibility, "visibility")?;
        let value = self
            .jira_client()?
            .add_comment(args.issue_key, args.body, visibility)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Edit a Jira issue comment")]
    async fn jira_edit_comment(
        &self,
        Parameters(args): Parameters<JiraEditCommentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let visibility = parse_optional_object_arg(args.visibility, "visibility")?;
        let value = self
            .jira_client()?
            .edit_comment(args.issue_key, args.comment_id, args.body, visibility)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get available transitions for a Jira issue")]
    async fn jira_get_transitions(
        &self,
        Parameters(args): Parameters<JiraGetTransitionsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_transitions(args.issue_key)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Transition a Jira issue")]
    async fn jira_transition_issue(
        &self,
        Parameters(args): Parameters<JiraTransitionIssueArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let fields = parse_optional_object_arg(args.fields, "fields")?;
        let value = self
            .jira_client()?
            .transition_issue(args.issue_key, args.transition_id, fields, args.comment)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Create a Jira issue")]
    async fn jira_create_issue(
        &self,
        Parameters(args): Parameters<JiraCreateIssueArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let deployment = self
            .context
            .jira_config()
            .ok_or_else(|| ErrorData::invalid_params("Jira is not configured", None))?
            .deployment;
        let fields = create_issue_fields_from_args(args, deployment)?;
        let value = self
            .jira_client()?
            .create_issue(fields)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Create multiple Jira issues in a batch")]
    async fn jira_batch_create_issues(
        &self,
        Parameters(args): Parameters<JiraBatchCreateIssuesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let deployment = self
            .context
            .jira_config()
            .ok_or_else(|| ErrorData::invalid_params("Jira is not configured", None))?
            .deployment;
        let issue_updates = batch_create_issue_updates_from_args(args.issues, deployment)?;
        let value = self
            .jira_client()?
            .batch_create_issues(issue_updates, args.validate_only.unwrap_or(false))
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get changelogs for multiple Jira issues")]
    async fn jira_batch_get_changelogs(
        &self,
        Parameters(args): Parameters<JiraBatchGetChangelogsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let issue_ids_or_keys =
            parse_required_string_list_arg(args.issue_ids_or_keys, "issue_ids_or_keys")?;
        let fields = parse_optional_string_list_arg(args.fields, "fields")?;
        let limit = optional_positive_i64_arg(args.limit, "limit")?;
        let value = self
            .jira_client()?
            .batch_get_changelogs(issue_ids_or_keys, fields, limit)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Update fields on a Jira issue")]
    async fn jira_update_issue(
        &self,
        Parameters(args): Parameters<JiraUpdateIssueArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let deployment = self
            .context
            .jira_config()
            .ok_or_else(|| ErrorData::invalid_params("Jira is not configured", None))?
            .deployment;
        let (fields, additional_fields) = update_issue_fields_from_args(args, deployment)?;
        let value = self
            .jira_client()?
            .update_issue(
                fields.issue_key,
                fields.fields,
                additional_fields,
                fields.notify_users,
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Delete a Jira issue")]
    async fn jira_delete_issue(
        &self,
        Parameters(args): Parameters<JiraDeleteIssueArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .delete_issue(args.issue_key, args.delete_subtasks.unwrap_or(false))
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "List Jira projects visible to the current user")]
    async fn jira_get_all_projects(
        &self,
        Parameters(args): Parameters<JiraGetAllProjectsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_all_projects(args.include_archived.unwrap_or(false))
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "List versions for a Jira project")]
    async fn jira_get_project_versions(
        &self,
        Parameters(args): Parameters<JiraGetProjectVersionsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_project_versions(args.project_key)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "List components for a Jira project")]
    async fn jira_get_project_components(
        &self,
        Parameters(args): Parameters<JiraGetProjectComponentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_project_components(args.project_key)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Create a Jira project version")]
    async fn jira_create_version(
        &self,
        Parameters(args): Parameters<JiraCreateVersionArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .create_version(version_payload_from_args(args)?)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Create multiple Jira project versions")]
    async fn jira_batch_create_versions(
        &self,
        Parameters(args): Parameters<JiraBatchCreateVersionsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let project_key = required_non_empty_arg(args.project_key, "project_key")?;
        let versions = parse_required_object_list_arg(args.versions, "versions")?
            .into_iter()
            .map(|version| version_payload_from_value(version, &project_key))
            .collect::<Result<Vec<_>, _>>()?;
        let value = self
            .jira_client()?
            .batch_create_versions(versions)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Retrieve a Jira user profile")]
    async fn jira_get_user_profile(
        &self,
        Parameters(args): Parameters<JiraGetUserProfileArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_user_profile(required_non_empty_arg(
                args.user_identifier,
                "user_identifier",
            )?)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get watchers for a Jira issue")]
    async fn jira_get_issue_watchers(
        &self,
        Parameters(args): Parameters<JiraGetIssueWatchersArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_issue_watchers(args.issue_key)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Add a watcher to a Jira issue")]
    async fn jira_add_watcher(
        &self,
        Parameters(args): Parameters<JiraAddWatcherArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .add_watcher(
                args.issue_key,
                required_non_empty_arg(args.user_identifier, "user_identifier")?,
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Remove a watcher from a Jira issue")]
    async fn jira_remove_watcher(
        &self,
        Parameters(args): Parameters<JiraRemoveWatcherArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .remove_watcher(
                args.issue_key,
                required_non_empty_arg(args.user_identifier, "user_identifier")?,
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get worklogs for a Jira issue")]
    async fn jira_get_worklog(
        &self,
        Parameters(args): Parameters<JiraGetWorklogArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_worklog(args.issue_key, args.start_at, args.limit)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Add a worklog entry to a Jira issue")]
    async fn jira_add_worklog(
        &self,
        Parameters(args): Parameters<JiraAddWorklogArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let deployment = self
            .context
            .jira_config()
            .ok_or_else(|| ErrorData::invalid_params("Jira is not configured", None))?
            .deployment;
        let (issue_key, payload, query) = add_worklog_payload_from_args(args, deployment)?;
        let value = self
            .jira_client()?
            .add_worklog(issue_key, payload, query)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get Jira issue link types")]
    async fn jira_get_link_types(
        &self,
        Parameters(args): Parameters<JiraGetLinkTypesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut value = self
            .jira_client()?
            .get_link_types()
            .await
            .map_err(jira_error)?;

        if let Some(name_filter) = optional_non_empty_arg(args.name_filter) {
            let name_filter = name_filter.to_lowercase();
            if let Some(link_types) = value
                .get_mut("issueLinkTypes")
                .and_then(Value::as_array_mut)
            {
                link_types.retain(|link_type| {
                    link_type
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.to_lowercase().contains(&name_filter))
                });
            }
        }

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Link a Jira issue to an epic using parent key")]
    async fn jira_link_to_epic(
        &self,
        Parameters(args): Parameters<JiraLinkToEpicArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .link_to_epic(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                required_non_empty_arg(args.epic_key, "epic_key")?,
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Create a link between two Jira issues")]
    async fn jira_create_issue_link(
        &self,
        Parameters(args): Parameters<JiraCreateIssueLinkArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let deployment = self
            .context
            .jira_config()
            .ok_or_else(|| ErrorData::invalid_params("Jira is not configured", None))?
            .deployment;
        let value = self
            .jira_client()?
            .create_issue_link(issue_link_payload_from_args(args, deployment)?)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Create a remote link on a Jira issue")]
    async fn jira_create_remote_issue_link(
        &self,
        Parameters(args): Parameters<JiraCreateRemoteIssueLinkArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (issue_key, payload) = remote_issue_link_payload_from_args(args)?;
        let value = self
            .jira_client()?
            .create_remote_issue_link(issue_key, payload)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Remove a Jira issue link by id")]
    async fn jira_remove_issue_link(
        &self,
        Parameters(args): Parameters<JiraRemoveIssueLinkArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .remove_issue_link(required_non_empty_arg(args.link_id, "link_id")?)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Download Jira issue attachments with bounded safe content output")]
    async fn jira_download_attachments(
        &self,
        Parameters(args): Parameters<JiraDownloadAttachmentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let attachment_ids = parse_optional_string_list_arg(args.attachment_ids, "attachment_ids")?;
        let max_bytes = optional_positive_u64_arg(args.max_bytes, "max_bytes")?
            .unwrap_or(DEFAULT_ATTACHMENT_MAX_BYTES);
        let value = self
            .jira_client()?
            .get_safe_issue_attachments(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                AttachmentFetchOptions {
                    attachment_ids,
                    include_content: args.include_content.unwrap_or(false),
                    images_only: false,
                    max_bytes,
                },
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get image attachments for a Jira issue with safe content output")]
    async fn jira_get_issue_images(
        &self,
        Parameters(args): Parameters<JiraGetIssueImagesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let max_bytes = optional_positive_u64_arg(args.max_bytes, "max_bytes")?
            .unwrap_or(DEFAULT_ATTACHMENT_MAX_BYTES);
        let value = self
            .jira_client()?
            .get_safe_issue_attachments(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                AttachmentFetchOptions {
                    attachment_ids: None,
                    include_content: args.include_content.unwrap_or(false),
                    images_only: true,
                    max_bytes,
                },
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get Jira Software agile boards")]
    async fn jira_get_agile_boards(
        &self,
        Parameters(args): Parameters<JiraGetAgileBoardsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_agile_boards(args.project_key, args.board_type, args.start_at, args.limit)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get issues on a Jira Software agile board")]
    async fn jira_get_board_issues(
        &self,
        Parameters(args): Parameters<JiraGetBoardIssuesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let fields = parse_optional_string_list_arg(args.fields, "fields")?;
        let value = self
            .jira_client()?
            .get_board_issues(args.board_id, args.jql, fields, args.start_at, args.limit)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get sprints for a Jira Software agile board")]
    async fn jira_get_sprints_from_board(
        &self,
        Parameters(args): Parameters<JiraGetSprintsFromBoardArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = parse_optional_string_list_arg(args.state, "state")?;
        let value = self
            .jira_client()?
            .get_sprints_from_board(args.board_id, state, args.start_at, args.limit)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get issues for a Jira Software sprint")]
    async fn jira_get_sprint_issues(
        &self,
        Parameters(args): Parameters<JiraGetSprintIssuesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let fields = parse_optional_string_list_arg(args.fields, "fields")?;
        let value = self
            .jira_client()?
            .get_sprint_issues(args.sprint_id, fields, args.start_at, args.limit)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Create a Jira Software sprint")]
    async fn jira_create_sprint(
        &self,
        Parameters(args): Parameters<JiraCreateSprintArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .create_sprint(create_sprint_payload_from_args(args)?)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Update a Jira Software sprint")]
    async fn jira_update_sprint(
        &self,
        Parameters(args): Parameters<JiraUpdateSprintArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (sprint_id, payload) = update_sprint_payload_from_args(args)?;
        let value = self
            .jira_client()?
            .update_sprint(sprint_id, payload)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Add Jira issues to a sprint")]
    async fn jira_add_issues_to_sprint(
        &self,
        Parameters(args): Parameters<JiraAddIssuesToSprintArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let issue_keys = parse_required_string_list_arg(args.issue_keys, "issue_keys")?;
        let value = self
            .jira_client()?
            .add_issues_to_sprint(args.sprint_id, issue_keys)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get the Jira Service Management service desk for a project")]
    async fn jira_get_service_desk_for_project(
        &self,
        Parameters(args): Parameters<JiraGetServiceDeskForProjectArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_service_desk_for_project(required_non_empty_arg(args.project_key, "project_key")?)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get queues for a Jira Service Management service desk")]
    async fn jira_get_service_desk_queues(
        &self,
        Parameters(args): Parameters<JiraGetServiceDeskQueuesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_service_desk_queues(args.service_desk_id, args.start_at, args.limit)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get issues for a Jira Service Management queue")]
    async fn jira_get_queue_issues(
        &self,
        Parameters(args): Parameters<JiraGetQueueIssuesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_queue_issues(
                args.service_desk_id,
                args.queue_id,
                args.start_at,
                args.limit,
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get Jira Forms or ProForma forms for an issue")]
    async fn jira_get_issue_proforma_forms(
        &self,
        Parameters(args): Parameters<JiraGetIssueProformaFormsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_issue_proforma_forms(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                self.context.atlassian_oauth_cloud_id(),
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get details for a Jira Form or ProForma form")]
    async fn jira_get_proforma_form_details(
        &self,
        Parameters(args): Parameters<JiraGetProformaFormDetailsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_proforma_form_details(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                required_non_empty_arg(args.form_id, "form_id")?,
                self.context.atlassian_oauth_cloud_id(),
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Update answers on a Jira Form or ProForma form")]
    async fn jira_update_proforma_form_answers(
        &self,
        Parameters(args): Parameters<JiraUpdateProformaFormAnswersArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let answers = parse_required_object_list_arg(args.answers, "answers")?;
        let value = self
            .jira_client()?
            .update_proforma_form_answers(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                required_non_empty_arg(args.form_id, "form_id")?,
                answers,
                self.context.atlassian_oauth_cloud_id(),
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get Jira issue date and status timing information")]
    async fn jira_get_issue_dates(
        &self,
        Parameters(args): Parameters<JiraGetIssueDatesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_issue_dates(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                args.include_status_changes.unwrap_or(false),
                args.include_status_summary.unwrap_or(false),
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get Jira Service Management SLA metrics for an issue")]
    async fn jira_get_issue_sla(
        &self,
        Parameters(args): Parameters<JiraGetIssueSlaArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let metrics = parse_optional_string_list_arg(args.metrics, "metrics")?;
        let value = self
            .jira_client()?
            .get_issue_sla(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                metrics,
                args.working_hours_only,
                args.include_raw_dates.unwrap_or(false),
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get Jira development information for an issue")]
    async fn jira_get_issue_development_info(
        &self,
        Parameters(args): Parameters<JiraGetIssueDevelopmentInfoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let value = self
            .jira_client()?
            .get_issue_development_info(
                required_non_empty_arg(args.issue_key, "issue_key")?,
                args.application_type,
                args.data_type,
            )
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Get Jira development information for multiple issues")]
    async fn jira_get_issues_development_info(
        &self,
        Parameters(args): Parameters<JiraGetIssuesDevelopmentInfoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let issue_keys = parse_required_string_list_arg(args.issue_keys, "issue_keys")?;
        let value = self
            .jira_client()?
            .get_issues_development_info(issue_keys, args.application_type, args.data_type)
            .await
            .map_err(jira_error)?;

        Ok(CallToolResult::structured(value))
    }
}

fn parse_optional_string_list_arg(
    value: Option<Value>,
    field_name: &'static str,
) -> Result<Option<Vec<String>>, ErrorData> {
    parse_optional_string_list(value, field_name).map_err(jira_error)
}

fn parse_required_string_list_arg(
    value: Value,
    field_name: &'static str,
) -> Result<Vec<String>, ErrorData> {
    parse_required_string_list(value, field_name).map_err(jira_error)
}

fn parse_optional_object_arg(
    value: Option<Value>,
    field_name: &'static str,
) -> Result<Option<Value>, ErrorData> {
    parse_optional_object(value, field_name).map_err(jira_error)
}

fn parse_required_object_arg(value: Value, field_name: &'static str) -> Result<Value, ErrorData> {
    parse_required_object(value, field_name).map_err(jira_error)
}

fn parse_required_object_list_arg(
    value: Value,
    field_name: &'static str,
) -> Result<Vec<Value>, ErrorData> {
    parse_required_object_list(value, field_name).map_err(jira_error)
}

fn create_issue_fields_from_args(
    args: JiraCreateIssueArgs,
    deployment: JiraDeployment,
) -> Result<Value, ErrorData> {
    let project_key = required_non_empty_arg(args.project_key, "project_key")?;
    let summary = required_non_empty_arg(args.summary, "summary")?;
    let issue_type = required_non_empty_arg(args.issue_type, "issue_type")?;
    let components = parse_optional_string_list_arg(args.components, "components")?;
    let additional_fields = parse_optional_object_arg(args.additional_fields, "additional_fields")?;
    let mut fields = json!({
        "project": {"key": project_key},
        "summary": summary,
        "issuetype": {"name": issue_type},
    });

    if let Some(description) = optional_non_empty_arg(args.description) {
        fields["description"] = comment_body_for_deployment(deployment, &description);
    }
    if let Some(assignee) = optional_non_empty_arg(args.assignee) {
        let identifier_field = match deployment {
            JiraDeployment::Cloud => "accountId",
            JiraDeployment::ServerDataCenter => "name",
        };
        fields["assignee"] = json!({ identifier_field: assignee });
    }
    if let Some(components) = components {
        let components = components
            .into_iter()
            .map(|name| json!({ "name": name }))
            .collect::<Vec<_>>();
        if !components.is_empty() {
            fields["components"] = Value::Array(components);
        }
    }

    merge_optional_objects(fields, additional_fields, "additional_fields").map_err(jira_error)
}

struct UpdateIssueFields {
    issue_key: String,
    fields: Value,
    notify_users: Option<bool>,
}

fn update_issue_fields_from_args(
    args: JiraUpdateIssueArgs,
    deployment: JiraDeployment,
) -> Result<(UpdateIssueFields, Option<Value>), ErrorData> {
    let issue_key = required_non_empty_arg(args.issue_key, "issue_key")?;
    let fields = normalize_issue_fields(
        parse_required_object_arg(args.fields, "fields")?,
        deployment,
        "fields",
    )?;
    let components = parse_optional_string_list_arg(args.components, "components")?;
    let mut additional_fields =
        parse_optional_object_arg(args.additional_fields, "additional_fields")?
            .map(|value| normalize_issue_fields(value, deployment, "additional_fields"))
            .transpose()?;

    reject_unsupported_attachments(&fields, "fields")?;
    if let Some(additional_fields) = additional_fields.as_ref() {
        reject_unsupported_attachments(additional_fields, "additional_fields")?;
    }

    if let Some(components) = components {
        let components = components
            .into_iter()
            .map(|name| json!({ "name": name }))
            .collect::<Vec<_>>();
        if !components.is_empty() {
            let additional = additional_fields.get_or_insert_with(|| json!({}));
            additional["components"] = Value::Array(components);
        }
    }

    if fields.as_object().is_some_and(Map::is_empty) && additional_fields.is_none() {
        return Err(jira_error(AtlassianError::invalid_input(
            "fields must contain at least one update",
        )));
    }

    Ok((
        UpdateIssueFields {
            issue_key,
            fields,
            notify_users: args.notify_users,
        },
        additional_fields,
    ))
}

fn normalize_issue_fields(
    mut fields: Value,
    deployment: JiraDeployment,
    field_name: &'static str,
) -> Result<Value, ErrorData> {
    reject_unsupported_attachments(&fields, field_name)?;
    let object = fields.as_object_mut().ok_or_else(|| {
        jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must be a JSON object"
        )))
    })?;

    if let Some(Value::String(description)) = object.get("description").cloned() {
        object.insert(
            "description".to_string(),
            comment_body_for_deployment(deployment, &description),
        );
    }
    if let Some(Value::String(assignee)) = object.get("assignee").cloned() {
        let identifier_field = match deployment {
            JiraDeployment::Cloud => "accountId",
            JiraDeployment::ServerDataCenter => "name",
        };
        object.insert(
            "assignee".to_string(),
            json!({ identifier_field: assignee }),
        );
    }

    Ok(fields)
}

fn reject_unsupported_attachments(
    value: &Value,
    field_name: &'static str,
) -> Result<(), ErrorData> {
    if value
        .as_object()
        .is_some_and(|object| object.contains_key("attachments"))
    {
        Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name}.attachments is not supported by jira_update_issue in Stage 3"
        ))))
    } else {
        Ok(())
    }
}

fn version_payload_from_args(args: JiraCreateVersionArgs) -> Result<Value, ErrorData> {
    let project_key = required_non_empty_arg(args.project_key, "project_key")?;
    let name = required_non_empty_arg(args.name, "name")?;
    let mut payload = json!({
        "project": project_key,
        "name": name,
    });
    insert_optional_value(
        &mut payload,
        "startDate",
        optional_non_empty_arg(args.start_date),
    );
    insert_optional_value(
        &mut payload,
        "releaseDate",
        optional_non_empty_arg(args.release_date),
    );
    insert_optional_value(
        &mut payload,
        "description",
        optional_non_empty_arg(args.description),
    );
    Ok(payload)
}

fn add_worklog_payload_from_args(
    args: JiraAddWorklogArgs,
    deployment: JiraDeployment,
) -> Result<(String, Value, Vec<(String, String)>), ErrorData> {
    let issue_key = required_non_empty_arg(args.issue_key, "issue_key")?;
    let time_spent = required_non_empty_arg(args.time_spent, "time_spent")?;
    let visibility = parse_optional_object_arg(args.visibility, "visibility")?;
    let mut payload = json!({
        "timeSpent": time_spent,
    });
    insert_optional_value(
        &mut payload,
        "started",
        optional_non_empty_arg(args.started),
    );
    if let Some(comment) = optional_non_empty_arg(args.comment) {
        payload["comment"] = comment_body_for_deployment(deployment, &comment);
    }
    if let Some(visibility) = visibility {
        payload["visibility"] = visibility;
    }

    let mut query = Vec::new();
    push_optional_query_value(&mut query, "adjustEstimate", args.adjust_estimate);
    push_optional_query_value(&mut query, "newEstimate", args.new_estimate);
    push_optional_query_value(&mut query, "reduceBy", args.reduce_by);
    Ok((issue_key, payload, query))
}

fn issue_link_payload_from_args(
    args: JiraCreateIssueLinkArgs,
    deployment: JiraDeployment,
) -> Result<Value, ErrorData> {
    let link_type = required_non_empty_arg(args.link_type, "link_type")?;
    let inward_issue_key = required_non_empty_arg(args.inward_issue_key, "inward_issue_key")?;
    let outward_issue_key = required_non_empty_arg(args.outward_issue_key, "outward_issue_key")?;
    let mut payload = json!({
        "type": {"name": link_type},
        "inwardIssue": {"key": inward_issue_key},
        "outwardIssue": {"key": outward_issue_key},
    });

    if let Some(comment) = optional_non_empty_arg(args.comment) {
        payload["comment"] = json!({
            "body": comment_body_for_deployment(deployment, &comment)
        });
    }

    Ok(payload)
}

fn remote_issue_link_payload_from_args(
    args: JiraCreateRemoteIssueLinkArgs,
) -> Result<(String, Value), ErrorData> {
    let issue_key = required_non_empty_arg(args.issue_key, "issue_key")?;
    let url = required_non_empty_arg(args.url, "url")?;
    let title = required_non_empty_arg(args.title, "title")?;
    let status = parse_optional_object_arg(args.status, "status")?;
    let mut object = json!({
        "url": url,
        "title": title,
    });
    insert_optional_value(&mut object, "summary", optional_non_empty_arg(args.summary));
    if let Some(icon_url) = optional_non_empty_arg(args.icon_url) {
        object["icon"] = json!({
            "url16x16": icon_url,
            "title": object["title"].clone(),
        });
    }
    if let Some(status) = status {
        object["status"] = status;
    }

    let mut payload = json!({ "object": object });
    insert_optional_value(
        &mut payload,
        "globalId",
        optional_non_empty_arg(args.global_id),
    );
    insert_optional_value(
        &mut payload,
        "relationship",
        optional_non_empty_arg(args.relationship),
    );
    Ok((issue_key, payload))
}

fn create_sprint_payload_from_args(args: JiraCreateSprintArgs) -> Result<Value, ErrorData> {
    let name = required_non_empty_arg(args.name, "name")?;
    let mut payload = json!({
        "name": name,
        "originBoardId": args.origin_board_id,
    });
    insert_optional_value(
        &mut payload,
        "startDate",
        optional_non_empty_arg(args.start_date),
    );
    insert_optional_value(
        &mut payload,
        "endDate",
        optional_non_empty_arg(args.end_date),
    );
    insert_optional_value(&mut payload, "goal", optional_non_empty_arg(args.goal));
    Ok(payload)
}

fn update_sprint_payload_from_args(args: JiraUpdateSprintArgs) -> Result<(u64, Value), ErrorData> {
    let mut payload = json!({});
    insert_optional_value(&mut payload, "name", optional_non_empty_arg(args.name));
    insert_optional_value(&mut payload, "state", optional_non_empty_arg(args.state));
    insert_optional_value(
        &mut payload,
        "startDate",
        optional_non_empty_arg(args.start_date),
    );
    insert_optional_value(
        &mut payload,
        "endDate",
        optional_non_empty_arg(args.end_date),
    );
    insert_optional_value(&mut payload, "goal", optional_non_empty_arg(args.goal));

    if payload.as_object().is_some_and(Map::is_empty) {
        return Err(jira_error(AtlassianError::invalid_input(
            "sprint update must contain at least one field",
        )));
    }

    Ok((args.sprint_id, payload))
}

fn version_payload_from_value(value: Value, project_key: &str) -> Result<Value, ErrorData> {
    let mut object = value_into_object(value, "version")?;
    let name = take_required_string_field(&mut object, "name")?;
    let start_date = take_optional_string_alias(&mut object, "startDate", "start_date")?;
    let release_date = take_optional_string_alias(&mut object, "releaseDate", "release_date")?;
    let description = take_optional_string_field(&mut object, "description")?;
    let mut payload = Value::Object(object);
    payload["project"] = Value::String(project_key.to_string());
    payload["name"] = Value::String(name);
    insert_optional_value(&mut payload, "startDate", start_date);
    insert_optional_value(&mut payload, "releaseDate", release_date);
    insert_optional_value(&mut payload, "description", description);
    Ok(payload)
}

fn take_optional_string_alias(
    object: &mut Map<String, Value>,
    first: &'static str,
    second: &'static str,
) -> Result<Option<String>, ErrorData> {
    match take_optional_string_field(object, first)? {
        Some(value) => Ok(Some(value)),
        None => take_optional_string_field(object, second),
    }
}

fn insert_optional_value(payload: &mut Value, key: &'static str, value: Option<String>) {
    if let Some(value) = value {
        payload[key] = Value::String(value);
    }
}

fn push_optional_query_value(
    query: &mut Vec<(String, String)>,
    key: &'static str,
    value: Option<String>,
) {
    if let Some(value) = optional_non_empty_arg(value) {
        query.push((key.to_string(), value));
    }
}

fn batch_create_issue_updates_from_args(
    issues: Value,
    deployment: JiraDeployment,
) -> Result<Vec<Value>, ErrorData> {
    parse_required_object_list_arg(issues, "issues")?
        .into_iter()
        .map(|issue| {
            create_issue_fields_from_value(issue, deployment)
                .map(|fields| json!({ "fields": fields }))
        })
        .collect()
}

fn create_issue_fields_from_value(
    issue: Value,
    deployment: JiraDeployment,
) -> Result<Value, ErrorData> {
    let mut fields = value_into_object(issue, "issue")?;
    let project_key = take_required_string_field(&mut fields, "project_key")?;
    let summary = take_required_string_field(&mut fields, "summary")?;
    let issue_type = take_required_string_field(&mut fields, "issue_type")?;
    let assignee = take_optional_string_field(&mut fields, "assignee")?;
    let description = take_optional_string_field(&mut fields, "description")?;
    let components = fields.remove("components");
    let additional_fields = if fields.is_empty() {
        None
    } else {
        Some(Value::Object(fields))
    };

    create_issue_fields_from_args(
        JiraCreateIssueArgs {
            project_key,
            summary,
            issue_type,
            assignee,
            description,
            components,
            additional_fields,
        },
        deployment,
    )
}

fn value_into_object(
    value: Value,
    field_name: &'static str,
) -> Result<Map<String, Value>, ErrorData> {
    match parse_required_object_arg(value, field_name)? {
        Value::Object(object) => Ok(object),
        _ => unreachable!("parse_required_object_arg only returns JSON objects"),
    }
}

fn take_required_string_field(
    object: &mut Map<String, Value>,
    field_name: &'static str,
) -> Result<String, ErrorData> {
    match object.remove(field_name) {
        Some(Value::String(value)) => required_non_empty_arg(value, field_name),
        Some(_) => Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must be a string"
        )))),
        None => Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} is required"
        )))),
    }
}

fn take_optional_string_field(
    object: &mut Map<String, Value>,
    field_name: &'static str,
) -> Result<Option<String>, ErrorData> {
    match object.remove(field_name) {
        Some(Value::String(value)) => Ok(optional_non_empty_arg(Some(value))),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must be a string"
        )))),
    }
}

fn required_non_empty_arg(value: String, field_name: &'static str) -> Result<String, ErrorData> {
    let value = value.trim();
    if value.is_empty() {
        Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must not be empty"
        ))))
    } else {
        Ok(value.to_string())
    }
}

fn optional_non_empty_arg(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn optional_positive_i64_arg(
    value: Option<i64>,
    field_name: &'static str,
) -> Result<Option<i64>, ErrorData> {
    match value {
        Some(value) if value <= 0 => Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must be positive"
        )))),
        value => Ok(value),
    }
}

fn optional_positive_u64_arg(
    value: Option<u64>,
    field_name: &'static str,
) -> Result<Option<u64>, ErrorData> {
    match value {
        Some(0) => Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must be positive"
        )))),
        value => Ok(value),
    }
}

fn optional_confluence_search_limit_arg(value: Option<u64>) -> Result<Option<u64>, ErrorData> {
    match value {
        Some(0) => Err(jira_error(AtlassianError::invalid_input(
            "limit must be positive",
        ))),
        Some(value) if value > crate::confluence::client::MAX_SEARCH_LIMIT => {
            Err(jira_error(AtlassianError::invalid_input(format!(
                "limit must be less than or equal to {}",
                crate::confluence::client::MAX_SEARCH_LIMIT
            ))))
        }
        value => Ok(value),
    }
}

fn optional_u64_range_arg(
    value: Option<u64>,
    default: u64,
    max: u64,
    field_name: &'static str,
) -> Result<u64, ErrorData> {
    match value.unwrap_or(default) {
        0 => Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must be positive"
        )))),
        value if value > max => Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must be less than or equal to {max}"
        )))),
        value => Ok(value),
    }
}

fn confluence_page_tool_value(
    page: &ConfluencePage,
    include_metadata: bool,
    convert_to_markdown: bool,
) -> Value {
    let simplified = page.to_simplified_value(convert_to_markdown);
    if include_metadata {
        json!({ "metadata": simplified })
    } else {
        json!({ "content": { "value": simplified.get("content").cloned().unwrap_or(Value::Null) } })
    }
}

fn parse_confluence_write_content_format(
    value: Option<&str>,
) -> Result<ConfluenceContentFormat, ErrorData> {
    let format = ConfluenceContentFormat::parse(value).map_err(jira_error)?;
    if format == ConfluenceContentFormat::Html {
        return Err(jira_error(AtlassianError::invalid_input(
            "content_format must be markdown, wiki, or storage",
        )));
    }
    Ok(format)
}

fn confluence_user_search_limit(value: Option<u64>) -> Result<u64, ErrorData> {
    match value.unwrap_or(10) {
        0 => Err(jira_error(AtlassianError::invalid_input(
            "limit must be positive",
        ))),
        value if value > 50 => Err(jira_error(AtlassianError::invalid_input(
            "limit must be less than or equal to 50",
        ))),
        value => Ok(value),
    }
}

fn confluence_positive_version_arg(value: u64, field_name: &'static str) -> Result<u64, ErrorData> {
    if value == 0 {
        Err(jira_error(AtlassianError::invalid_input(format!(
            "{field_name} must be positive"
        ))))
    } else {
        Ok(value)
    }
}

fn normalize_confluence_user_search_query(query: &str) -> String {
    if ["=", "~", ">", "<", " AND ", " OR ", "user."]
        .iter()
        .any(|token| query.contains(token))
    {
        query.to_string()
    } else {
        format!(
            "user.fullname ~ \"{}\"",
            query.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

fn confluence_page_markdown_content(page: &ConfluencePage) -> String {
    page.to_simplified_value(true)
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn confluence_unified_diff(
    from_content: &str,
    to_content: &str,
    from_version: u64,
    to_version: u64,
) -> String {
    if from_content == to_content {
        return String::new();
    }

    let from_lines = from_content.lines().collect::<Vec<_>>();
    let to_lines = to_content.lines().collect::<Vec<_>>();
    let mut output = vec![
        format!("--- v{from_version}"),
        format!("+++ v{to_version}"),
        format!(
            "@@ -{} +{} @@",
            confluence_diff_range(from_lines.len()),
            confluence_diff_range(to_lines.len())
        ),
    ];
    let max_len = from_lines.len().max(to_lines.len());

    for index in 0..max_len {
        match (from_lines.get(index), to_lines.get(index)) {
            (Some(left), Some(right)) if left == right => output.push(format!(" {left}")),
            (Some(left), Some(right)) => {
                output.push(format!("-{left}"));
                output.push(format!("+{right}"));
            }
            (Some(left), None) => output.push(format!("-{left}")),
            (None, Some(right)) => output.push(format!("+{right}")),
            (None, None) => {}
        }
    }

    output.join("\n")
}

fn confluence_diff_range(line_count: usize) -> String {
    match line_count {
        0 => "0,0".to_string(),
        1 => "1".to_string(),
        value => format!("1,{value}"),
    }
}

async fn confluence_attachment_with_content_value(
    client: &ConfluenceClient,
    attachment: &ConfluenceAttachment,
    fallback_id: &str,
    max_bytes: u64,
) -> Result<Value, Value> {
    let attachment_id = confluence_attachment_id_with_fallback(attachment, fallback_id);
    let filename = confluence_attachment_filename(attachment, &attachment_id);
    let mut value = attachment.to_simplified_value();

    if let Some(file_size) = attachment.file_size()
        && file_size > max_bytes
    {
        return Err(json!({
            "success": false,
            "attachment_id": attachment_id,
            "filename": filename,
            "file_size": file_size,
            "max_bytes": max_bytes,
            "error": format!("Attachment '{filename}' is {file_size} bytes which exceeds the inline limit of {max_bytes} bytes."),
        }));
    }

    let Some(download_url) = attachment
        .links
        .get("download")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(json!({
            "success": false,
            "attachment_id": attachment_id,
            "filename": filename,
            "error": "download URL is missing",
        }));
    };

    let content = client
        .download_relative_or_same_origin(download_url, max_bytes)
        .await
        .map_err(|error| {
            json!({
                "success": false,
                "attachment_id": attachment_id,
                "filename": filename,
                "error": redact_url_query(&error.to_string()),
            })
        })?;
    let content_type = content
        .content_type
        .clone()
        .filter(|content_type| !confluence_is_ambiguous_mime_type(Some(content_type.as_str())))
        .or_else(|| attachment.media_type().map(ToString::to_string))
        .or_else(|| confluence_guess_mime_from_filename(&filename).map(ToString::to_string))
        .unwrap_or_else(|| "application/octet-stream".to_string());

    value["content"] = json!({
        "encoding": "base64",
        "content_type": content_type,
        "size": content.bytes.len(),
        "data": base64_encode(&content.bytes),
    });

    Ok(value)
}

fn confluence_split_file_paths(value: &str) -> Result<Vec<String>, ErrorData> {
    let file_paths = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if file_paths.is_empty() {
        Err(jira_error(AtlassianError::invalid_input(
            "file_paths must contain at least one local file path",
        )))
    } else {
        Ok(file_paths)
    }
}

fn confluence_file_path_display(value: &str) -> String {
    Path::new(value)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment")
        .to_string()
}

fn confluence_attachment_id(attachment: &ConfluenceAttachment) -> String {
    confluence_attachment_id_with_fallback(attachment, "unknown")
}

fn confluence_attachment_id_with_fallback(
    attachment: &ConfluenceAttachment,
    fallback_id: &str,
) -> String {
    attachment
        .id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback_id.to_string())
}

fn confluence_attachment_filename(attachment: &ConfluenceAttachment, fallback_id: &str) -> String {
    attachment
        .title
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback_id.to_string())
}

fn confluence_is_image_attachment(media_type: Option<&str>, filename: &str) -> (bool, String) {
    if let Some(media_type) = media_type
        && matches!(
            media_type,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/svg+xml" | "image/bmp"
        )
    {
        return (true, media_type.to_string());
    }

    if media_type.is_none() || confluence_is_ambiguous_mime_type(media_type) {
        if let Some(guessed) = confluence_guess_mime_from_filename(filename)
            && guessed.starts_with("image/")
        {
            return (true, guessed.to_string());
        }
    }

    (
        false,
        media_type.unwrap_or("application/octet-stream").to_string(),
    )
}

fn confluence_is_ambiguous_mime_type(media_type: Option<&str>) -> bool {
    matches!(
        media_type,
        Some("application/octet-stream" | "application/binary")
    )
}

fn confluence_guess_mime_from_filename(filename: &str) -> Option<&'static str> {
    let filename = filename.to_ascii_lowercase();
    if filename.ends_with(".png") {
        Some("image/png")
    } else if filename.ends_with(".jpg") || filename.ends_with(".jpeg") {
        Some("image/jpeg")
    } else if filename.ends_with(".gif") {
        Some("image/gif")
    } else if filename.ends_with(".webp") {
        Some("image/webp")
    } else if filename.ends_with(".svg") {
        Some("image/svg+xml")
    } else if filename.ends_with(".bmp") {
        Some("image/bmp")
    } else if filename.ends_with(".txt") {
        Some("text/plain")
    } else if filename.ends_with(".pdf") {
        Some("application/pdf")
    } else {
        None
    }
}

fn confluence_write_page_value(page: &ConfluencePage, include_content: bool) -> Value {
    let mut value = page.to_simplified_value(false);
    if !include_content && let Some(object) = value.as_object_mut() {
        object.remove("content");
    }
    value
}

fn confluence_expand_list(expand: Option<String>, include_content: bool) -> Vec<String> {
    let mut values = expand
        .unwrap_or_else(|| "version".to_string())
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if values.is_empty() {
        values.push("version".to_string());
    }
    if include_content && !values.iter().any(|value| value.contains("body")) {
        values.push("body.storage".to_string());
    }

    values
}

fn confluence_child_page_value(
    page: &ConfluencePage,
    include_content: bool,
    convert_to_markdown: bool,
) -> Value {
    let mut value = page.to_simplified_value(convert_to_markdown);
    if !include_content && let Some(object) = value.as_object_mut() {
        object.remove("content");
    }
    value
}

#[derive(Debug)]
struct ConfluenceTreePageSortValue {
    depth: usize,
    position_sort: i64,
    title: String,
    value: Value,
}

fn confluence_tree_page_sort_value(page: &ConfluencePage) -> ConfluenceTreePageSortValue {
    let depth = page.ancestors.len();
    let parent_id = page
        .ancestors
        .last()
        .and_then(|ancestor| ancestor.id.clone());
    let position = page.extensions.get("position").and_then(Value::as_i64);
    let title = page
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "Untitled".to_string());
    let value = json!({
        "id": page.id.clone(),
        "title": title,
        "parent_id": parent_id,
        "position": position,
        "depth": depth,
    });

    ConfluenceTreePageSortValue {
        depth,
        position_sort: position.unwrap_or(999_999),
        title,
        value,
    }
}

fn jira_error(error: AtlassianError) -> ErrorData {
    match error {
        AtlassianError::InvalidInput { .. } => ErrorData::invalid_params(error.to_string(), None),
        _ => ErrorData::internal_error(error.to_string(), None),
    }
}

const TOOL_LOG_REDACTED: &str = "[redacted]";
const TOOL_LOG_TRUNCATED: &str = "[truncated]";
const TOOL_LOG_MAX_DEPTH: usize = 8;
const TOOL_LOG_MAX_ARRAY_ITEMS: usize = 50;
const TOOL_LOG_MAX_STRING_CHARS: usize = 1_000;

fn sanitize_tool_log_arguments(arguments: Option<&Map<String, Value>>) -> Value {
    arguments.map_or_else(
        || Value::Object(Map::new()),
        |arguments| Value::Object(sanitize_tool_log_object(arguments, 0)),
    )
}

fn sanitize_tool_log_object(arguments: &Map<String, Value>, depth: usize) -> Map<String, Value> {
    arguments
        .iter()
        .map(|(key, value)| {
            let value = if is_sensitive_log_key(key) {
                Value::String(TOOL_LOG_REDACTED.to_string())
            } else {
                sanitize_tool_log_value(value, depth + 1)
            };
            (key.clone(), value)
        })
        .collect()
}

fn sanitize_tool_log_value(value: &Value, depth: usize) -> Value {
    if depth > TOOL_LOG_MAX_DEPTH {
        return Value::String(TOOL_LOG_TRUNCATED.to_string());
    }

    match value {
        Value::Array(values) => {
            let mut sanitized = values
                .iter()
                .take(TOOL_LOG_MAX_ARRAY_ITEMS)
                .map(|value| sanitize_tool_log_value(value, depth + 1))
                .collect::<Vec<_>>();
            if values.len() > TOOL_LOG_MAX_ARRAY_ITEMS {
                sanitized.push(json!({
                    "truncated_items": values.len() - TOOL_LOG_MAX_ARRAY_ITEMS,
                }));
            }
            Value::Array(sanitized)
        }
        Value::Object(object) => Value::Object(sanitize_tool_log_object(object, depth + 1)),
        Value::String(value) => Value::String(truncate_tool_log_string(value)),
        value => value.clone(),
    }
}

fn is_sensitive_log_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    if matches!(
        key.as_str(),
        "page_token" | "next_page_token" | "nextpagetoken"
    ) {
        return false;
    }

    [
        "authorization",
        "cookie",
        "password",
        "secret",
        "token",
        "api_token",
        "personal_token",
        "pat",
    ]
    .iter()
    .any(|sensitive| key.contains(sensitive))
}

fn truncate_tool_log_string(value: &str) -> String {
    if value.chars().count() <= TOOL_LOG_MAX_STRING_CHARS {
        return value.to_string();
    }

    let mut truncated = value
        .chars()
        .take(TOOL_LOG_MAX_STRING_CHARS)
        .collect::<String>();
    truncated.push_str(TOOL_LOG_TRUNCATED);
    truncated
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AtlassianMcpServer {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let tool_name = request.name.to_string();
        let debug_arguments = tracing::enabled!(tracing::Level::DEBUG)
            .then(|| sanitize_tool_log_arguments(request.arguments.as_ref()));
        let started_at = Instant::now();

        if let Some(arguments) = debug_arguments.as_ref() {
            tracing::debug!(
                tool = %tool_name,
                arguments = %arguments,
                "MCP tool call started"
            );
        }

        let result = async {
            self.guard_registered_tool_call(tool_name.as_str())?;

            let tool_call_context = ToolCallContext::new(self, request, context);
            self.tool_router.call(tool_call_context).await
        }
        .await;
        let elapsed_ms = started_at.elapsed().as_millis();

        match &result {
            Ok(_) => {
                tracing::debug!(
                    tool = %tool_name,
                    elapsed_ms,
                    "MCP tool call completed"
                );
            }
            Err(error) => {
                tracing::warn!(
                    tool = %tool_name,
                    "MCP tool call failed"
                );
                if let Some(arguments) = debug_arguments.as_ref() {
                    tracing::debug!(
                        tool = %tool_name,
                        arguments = %arguments,
                        error_code = error.code.0,
                        error_message = %error.message,
                        elapsed_ms,
                        "MCP tool call failed details"
                    );
                }
            }
        }

        result
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(self.current_tools_result())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router
            .get(name)
            .cloned()
            .filter(|tool| !self.filtered_tools_from([tool.clone()]).is_empty())
            .map(sanitize_tool_for_clients)
    }

    fn get_info(&self) -> ServerInfo {
        let access_mode = if self.context.read_only() {
            "read-only"
        } else {
            "read/write"
        };

        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(format!(
                "Rust MCP Atlassian Stage 2 migration. The MCP control plane is initialized in {access_mode} mode. Jira core tools are available when Jira configuration and authentication are complete; Confluence tools are not migrated yet."
            ))
    }
}

fn sanitize_tools_for_clients(tools: Vec<Tool>) -> Vec<Tool> {
    tools.into_iter().map(sanitize_tool_for_clients).collect()
}

fn sanitize_tool_for_clients(mut tool: Tool) -> Tool {
    let mut schema = tool.input_schema.as_ref().clone();
    sanitize_schema_object(&mut schema);
    tool.input_schema = Arc::new(schema);
    tool
}

fn sanitize_schema_object(object: &mut Map<String, Value>) {
    if object.get("default").is_some_and(Value::is_null) {
        object.remove("default");
    }

    if let Some(type_value) = object.get_mut("type") {
        sanitize_type_value(type_value);
        if type_value.as_array().is_some_and(Vec::is_empty) {
            object.remove("type");
        }
    }

    for (key, value) in object.iter_mut() {
        if key == "additionalProperties" {
            continue;
        }
        sanitize_schema_value(value);
    }
}

fn sanitize_schema_value(value: &mut Value) {
    match value {
        Value::Bool(true) => {
            *value = json!({
                "type": "object",
                "additionalProperties": true
            });
        }
        Value::Bool(false) => {
            *value = json!({ "not": {} });
        }
        Value::Array(values) => {
            for value in values {
                sanitize_schema_value(value);
            }
        }
        Value::Object(object) => {
            if object.get("default").is_some_and(Value::is_null) {
                object.remove("default");
            }

            if let Some(type_value) = object.get_mut("type") {
                sanitize_type_value(type_value);
                if type_value.as_array().is_some_and(Vec::is_empty) {
                    object.remove("type");
                }
            }

            for (key, value) in object.iter_mut() {
                if key == "additionalProperties" {
                    continue;
                }
                sanitize_schema_value(value);
            }
        }
        Value::Null | Value::Number(_) | Value::String(_) => {}
    }
}

fn sanitize_type_value(type_value: &mut Value) {
    let Value::Array(types) = type_value else {
        return;
    };

    types.retain(|value| value.as_str() != Some("null"));
    if types.len() == 1 {
        *type_value = types[0].clone();
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, net::SocketAddr, sync::Arc};

    use crate::{
        atlassian::auth::AtlassianAuth,
        config::{HttpConfig, RuntimeConfig},
        confluence::config::{ConfluenceConfig, ConfluenceDeployment},
        context::AppContext,
        jira::config::{JiraConfig, JiraDeployment},
        jira::tools,
        tool_registry::{MIGRATION_STATUS_TOOL_NAME, ToolAccess, ToolMetadata, ToolService},
    };
    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, Method, StatusCode},
        response::{IntoResponse, Response},
        routing::any,
    };
    use rmcp::model::{JsonObject, Tool};
    use rmcp::{ServerHandler, handler::server::wrapper::Parameters};
    use serde_json::{Value, json};
    use tokio::sync::Mutex;

    use super::*;

    fn server_with_config(config: RuntimeConfig) -> AtlassianMcpServer {
        AtlassianMcpServer::new(Arc::new(AppContext::from_config(&config)))
    }

    const SYNTHETIC_JIRA_READ: ToolMetadata = ToolMetadata {
        name: "stage1_synthetic_jira_read",
        service: ToolService::Jira,
        access: ToolAccess::Read,
        toolset: Some("jira_issues"),
        title: "Synthetic Jira read",
        description: "Test-only Jira read metadata.",
    };

    const SYNTHETIC_JIRA_WRITE: ToolMetadata = ToolMetadata {
        name: "stage1_synthetic_jira_write",
        service: ToolService::Jira,
        access: ToolAccess::Write,
        toolset: Some("jira_issues"),
        title: "Synthetic Jira write",
        description: "Test-only Jira write metadata.",
    };

    const SYNTHETIC_CONFLUENCE_READ: ToolMetadata = ToolMetadata {
        name: "stage1_synthetic_confluence_read",
        service: ToolService::Confluence,
        access: ToolAccess::Read,
        toolset: Some("confluence_pages"),
        title: "Synthetic Confluence read",
        description: "Test-only Confluence read metadata.",
    };

    fn metadata_for_test_tool(name: &str) -> Option<ToolMetadata> {
        match name {
            "stage1_synthetic_jira_read" => Some(SYNTHETIC_JIRA_READ),
            "stage1_synthetic_jira_write" => Some(SYNTHETIC_JIRA_WRITE),
            "stage1_synthetic_confluence_read" => Some(SYNTHETIC_CONFLUENCE_READ),
            _ => tool_registry::metadata_for(name),
        }
    }

    fn runtime_config() -> RuntimeConfig {
        RuntimeConfig {
            http: HttpConfig::default(),
            ..RuntimeConfig::default()
        }
    }

    fn jira_config() -> JiraConfig {
        jira_config_with_base_url("https://jira.example".to_string())
    }

    fn confluence_config() -> ConfluenceConfig {
        confluence_config_with_base_url("https://confluence.example".to_string())
    }

    fn confluence_cloud_config_with_base_url(base_url: String) -> ConfluenceConfig {
        ConfluenceConfig {
            base_url,
            deployment: ConfluenceDeployment::Cloud,
            auth: AtlassianAuth::Basic {
                username: "test-user".to_string(),
                api_token: "test-api-token".to_string(),
            },
            ssl_verify: true,
            spaces_filter: BTreeSet::new(),
            timeout_seconds: 75,
        }
    }

    fn confluence_config_with_base_url(base_url: String) -> ConfluenceConfig {
        ConfluenceConfig {
            base_url,
            deployment: ConfluenceDeployment::ServerDataCenter,
            auth: AtlassianAuth::Pat {
                personal_token: "test-pat-value".to_string(),
            },
            ssl_verify: true,
            spaces_filter: BTreeSet::new(),
            timeout_seconds: 75,
        }
    }

    #[test]
    fn tool_log_arguments_redact_sensitive_fields_and_truncate_large_values() {
        let long_value = "x".repeat(TOOL_LOG_MAX_STRING_CHARS + 1);
        let mut nested = Map::new();
        nested.insert("password".to_string(), json!("secret-password"));
        let mut arguments = Map::new();
        arguments.insert("jql".to_string(), json!("project = ABC"));
        arguments.insert("page_token".to_string(), json!("visible-page-token"));
        arguments.insert("api_token".to_string(), json!("test-api-token"));
        arguments.insert("nested".to_string(), Value::Object(nested));
        arguments.insert("description".to_string(), Value::String(long_value));

        let sanitized = sanitize_tool_log_arguments(Some(&arguments));

        assert_eq!(sanitized["jql"], "project = ABC");
        assert_eq!(sanitized["page_token"], "visible-page-token");
        assert_eq!(sanitized["api_token"], TOOL_LOG_REDACTED);
        assert_eq!(sanitized["nested"]["password"], TOOL_LOG_REDACTED);
        let description = sanitized["description"].as_str().unwrap();
        assert!(description.ends_with(TOOL_LOG_TRUNCATED));
        assert!(!sanitized.to_string().contains("test-api-token"));
        assert!(!sanitized.to_string().contains("secret-password"));
    }

    fn jira_config_with_base_url(base_url: String) -> JiraConfig {
        JiraConfig {
            base_url,
            deployment: JiraDeployment::ServerDataCenter,
            auth: AtlassianAuth::Pat {
                personal_token: "test-pat-value".to_string(),
            },
            ssl_verify: true,
            projects_filter: BTreeSet::new(),
            timeout_seconds: 75,
        }
    }

    fn tool(name: &'static str) -> Tool {
        Tool::new(name, "", Arc::<JsonObject>::new(Default::default()))
    }

    fn current_tool_names(server: &AtlassianMcpServer) -> Vec<String> {
        tool_names(server.current_tools_result().tools)
    }

    fn tool_names(tools: Vec<Tool>) -> Vec<String> {
        tools
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    fn query_value(path: &str, key: &str) -> Option<String> {
        let url = reqwest::Url::parse(&format!("http://example{path}")).unwrap();
        url.query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
    }

    fn assert_client_compatible_tool_schemas(tools: &[Tool]) {
        for tool in tools {
            let schema = Value::Object(tool.input_schema.as_ref().clone());
            assert_client_compatible_schema_value(&schema, &tool.name);
            assert_explicit_property_schemas(&schema, &tool.name);
        }
    }

    fn assert_client_compatible_schema_value(value: &Value, path: &str) {
        match value {
            Value::Bool(_) => panic!("{path} contains a boolean JSON schema"),
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    assert_client_compatible_schema_value(value, &format!("{path}[{index}]"));
                }
            }
            Value::Object(object) => {
                if object.get("default").is_some_and(Value::is_null) {
                    panic!("{path} contains default: null");
                }
                if object
                    .get("type")
                    .and_then(Value::as_array)
                    .is_some_and(|types| types.iter().any(|value| value.as_str() == Some("null")))
                {
                    panic!("{path} contains nullable type array");
                }

                for (key, value) in object {
                    if key == "additionalProperties" {
                        continue;
                    }
                    assert_client_compatible_schema_value(value, &format!("{path}.{key}"));
                }
            }
            Value::Null | Value::Number(_) | Value::String(_) => {}
        }
    }

    fn assert_explicit_property_schemas(value: &Value, path: &str) {
        let Value::Object(object) = value else {
            return;
        };

        if let Some(properties) = object.get("properties").and_then(Value::as_object) {
            for (name, property_schema) in properties {
                let property_path = format!("{path}.properties.{name}");
                let Some(property) = property_schema.as_object() else {
                    panic!("{property_path} is not an object schema");
                };
                let has_explicit_shape = [
                    "type", "anyOf", "oneOf", "allOf", "$ref", "not", "const", "enum",
                ]
                .iter()
                .any(|key| property.contains_key(*key));
                assert!(has_explicit_shape, "{property_path} has no explicit shape");
            }
        }

        for (key, value) in object {
            if key == "additionalProperties" || key == "properties" {
                continue;
            }
            assert_explicit_property_schemas(value, &format!("{path}.{key}"));
        }
    }

    fn stage_three_candidate_tools() -> Vec<Tool> {
        tools::STAGE3_JIRA_TOOL_NAMES
            .iter()
            .map(|&name| tool(name))
            .collect()
    }

    fn stage_three_write_tool_names() -> Vec<&'static str> {
        tools::STAGE3_JIRA_TOOL_NAMES
            .iter()
            .copied()
            .filter(|name| {
                tool_registry::metadata_for(name)
                    .is_some_and(|metadata| metadata.access == ToolAccess::Write)
            })
            .collect()
    }

    fn stage_three_c3_tool_names() -> Vec<&'static str> {
        vec![
            tools::JIRA_GET_ALL_PROJECTS_TOOL_NAME,
            tools::JIRA_GET_PROJECT_VERSIONS_TOOL_NAME,
            tools::JIRA_GET_PROJECT_COMPONENTS_TOOL_NAME,
            tools::JIRA_CREATE_VERSION_TOOL_NAME,
            tools::JIRA_BATCH_CREATE_VERSIONS_TOOL_NAME,
            tools::JIRA_GET_USER_PROFILE_TOOL_NAME,
            tools::JIRA_GET_ISSUE_WATCHERS_TOOL_NAME,
            tools::JIRA_ADD_WATCHER_TOOL_NAME,
            tools::JIRA_REMOVE_WATCHER_TOOL_NAME,
            tools::JIRA_GET_WORKLOG_TOOL_NAME,
            tools::JIRA_ADD_WORKLOG_TOOL_NAME,
            tools::JIRA_GET_LINK_TYPES_TOOL_NAME,
            tools::JIRA_LINK_TO_EPIC_TOOL_NAME,
            tools::JIRA_CREATE_ISSUE_LINK_TOOL_NAME,
            tools::JIRA_CREATE_REMOTE_ISSUE_LINK_TOOL_NAME,
            tools::JIRA_REMOVE_ISSUE_LINK_TOOL_NAME,
            tools::JIRA_DOWNLOAD_ATTACHMENTS_TOOL_NAME,
            tools::JIRA_GET_ISSUE_IMAGES_TOOL_NAME,
        ]
    }

    fn stage_three_c3_write_tool_names() -> Vec<&'static str> {
        stage_three_c3_tool_names()
            .into_iter()
            .filter(|name| {
                tool_registry::metadata_for(name)
                    .is_some_and(|metadata| metadata.access == ToolAccess::Write)
            })
            .collect()
    }

    fn stage_three_c4_tool_names() -> Vec<&'static str> {
        vec![
            tools::JIRA_GET_AGILE_BOARDS_TOOL_NAME,
            tools::JIRA_GET_BOARD_ISSUES_TOOL_NAME,
            tools::JIRA_GET_SPRINTS_FROM_BOARD_TOOL_NAME,
            tools::JIRA_GET_SPRINT_ISSUES_TOOL_NAME,
            tools::JIRA_CREATE_SPRINT_TOOL_NAME,
            tools::JIRA_UPDATE_SPRINT_TOOL_NAME,
            tools::JIRA_ADD_ISSUES_TO_SPRINT_TOOL_NAME,
            tools::JIRA_GET_SERVICE_DESK_FOR_PROJECT_TOOL_NAME,
            tools::JIRA_GET_SERVICE_DESK_QUEUES_TOOL_NAME,
            tools::JIRA_GET_QUEUE_ISSUES_TOOL_NAME,
            tools::JIRA_GET_ISSUE_PROFORMA_FORMS_TOOL_NAME,
            tools::JIRA_GET_PROFORMA_FORM_DETAILS_TOOL_NAME,
            tools::JIRA_UPDATE_PROFORMA_FORM_ANSWERS_TOOL_NAME,
            tools::JIRA_GET_ISSUE_DATES_TOOL_NAME,
            tools::JIRA_GET_ISSUE_SLA_TOOL_NAME,
            tools::JIRA_GET_ISSUE_DEVELOPMENT_INFO_TOOL_NAME,
            tools::JIRA_GET_ISSUES_DEVELOPMENT_INFO_TOOL_NAME,
        ]
    }

    fn stage_three_c4_write_tool_names() -> Vec<&'static str> {
        stage_three_c4_tool_names()
            .into_iter()
            .filter(|name| {
                tool_registry::metadata_for(name)
                    .is_some_and(|metadata| metadata.access == ToolAccess::Write)
            })
            .collect()
    }

    fn expected_stage_two_default_tools() -> Vec<String> {
        vec![
            tools::JIRA_ADD_COMMENT_TOOL_NAME.to_string(),
            tools::JIRA_EDIT_COMMENT_TOOL_NAME.to_string(),
            tools::JIRA_GET_FIELD_OPTIONS_TOOL_NAME.to_string(),
            tools::JIRA_GET_ISSUE_TOOL_NAME.to_string(),
            tools::JIRA_GET_PROJECT_ISSUES_TOOL_NAME.to_string(),
            tools::JIRA_GET_TRANSITIONS_TOOL_NAME.to_string(),
            tools::JIRA_SEARCH_TOOL_NAME.to_string(),
            tools::JIRA_SEARCH_FIELDS_TOOL_NAME.to_string(),
            tools::JIRA_TRANSITION_ISSUE_TOOL_NAME.to_string(),
            MIGRATION_STATUS_TOOL_NAME.to_string(),
        ]
    }

    #[derive(Clone, Debug)]
    struct RecordedRequest {
        method: Method,
        path: String,
        body: Value,
    }

    #[derive(Clone)]
    struct MockJiraState {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    #[derive(Clone)]
    struct MockConfluenceState {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    async fn mock_jira_handler(
        State(state): State<MockJiraState>,
        method: Method,
        headers: HeaderMap,
        uri: axum::http::Uri,
        body: Bytes,
    ) -> Response {
        let parsed_body = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
        };
        let path = uri
            .path_and_query()
            .map(ToString::to_string)
            .unwrap_or_else(|| uri.path().to_string());
        state.requests.lock().await.push(RecordedRequest {
            method: method.clone(),
            path: path.clone(),
            body: parsed_body.clone(),
        });

        let expected_header = format!("Bearer {}", "test-pat-value");
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some(expected_header.as_str())
        {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"errorMessages": ["auth"]})),
            )
                .into_response();
        }

        let path_only = uri.path();
        if method == Method::GET && path_only == "/secure/attachment/1/file.png" {
            return (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "image/png")],
                "image-bytes",
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/secure/attachment/2/notes.txt" {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "errorMessages": [
                        "failed /secure/attachment/2/notes.txt?token=secret&client=abc"
                    ]
                })),
            )
                .into_response();
        }

        if method == Method::GET
            && (path == "/rest/api/2/issue/ABC-1" || path.starts_with("/rest/api/2/issue/ABC-1?"))
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "10001",
                    "key": "ABC-1",
                    "fields": {
                        "summary": "Mock issue",
                        "created": "2026-01-01T00:00:00.000+0000",
                        "updated": "2026-01-02T00:00:00.000+0000",
                        "duedate": "2026-01-10",
                        "resolutiondate": "2026-01-03T00:00:00.000+0000",
                        "status": {
                            "id": "3",
                            "name": "Done",
                            "statusCategory": {"name": "Done"}
                        },
                        "customfield_sla": {
                            "name": "Time to resolution SLA",
                            "ongoingCycle": {
                                "breached": false,
                                "elapsedTime": {"millis": 60000},
                                "remainingTime": {"millis": 120000},
                                "startTime": "2026-01-01T00:00:00.000+0000"
                            }
                        },
                        "attachment": [
                            {
                                "id": "1",
                                "filename": "file.png",
                                "mimeType": "image/png",
                                "size": 11,
                                "content": "/secure/attachment/1/file.png?token=secret"
                            },
                            {
                                "id": "2",
                                "filename": "notes.txt",
                                "mimeType": "text/plain",
                                "size": 42,
                                "content": "/secure/attachment/2/notes.txt?token=secret&client=abc"
                            }
                        ]
                    }
                })),
            )
                .into_response();
        }
        if method == Method::GET
            && (path == "/rest/api/2/issue/TXT-1" || path.starts_with("/rest/api/2/issue/TXT-1?"))
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "20001",
                    "key": "TXT-1",
                    "fields": {
                        "summary": "Text only",
                        "attachment": [
                            {
                                "id": "2",
                                "filename": "notes.txt",
                                "mimeType": "text/plain",
                                "size": 42,
                                "content": "/secure/attachment/2/notes.txt?token=secret&client=abc"
                            }
                        ]
                    }
                })),
            )
                .into_response();
        }
        if method == Method::GET && path == "/rest/api/2/issue/ABC-1/watchers" {
            return (
                StatusCode::OK,
                Json(json!({
                    "watchCount": 1,
                    "isWatching": false,
                    "watchers": [
                        {"accountId": "account-1", "displayName": "Ada Lovelace", "active": true}
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::POST && path == "/rest/api/2/issue/ABC-1/watchers" {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::DELETE && path == "/rest/api/2/issue/ABC-1/watchers?username=ada" {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::GET
            && path == "/rest/api/2/issue/ABC-1/worklog?startAt=0&maxResults=10"
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "startAt": 0,
                    "maxResults": 10,
                    "total": 2,
                    "worklogs": [
                        {
                            "id": "100",
                            "timeSpent": "1h",
                            "started": "2026-01-01T00:00:00.000+0000",
                            "author": {"displayName": "Ada Lovelace"}
                        },
                        {
                            "id": "101",
                            "timeSpent": "30m"
                        }
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::POST
            && path == "/rest/api/2/issue/ABC-1/worklog?adjustEstimate=new&newEstimate=2h"
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "300",
                    "timeSpent": parsed_body["timeSpent"],
                    "started": parsed_body["started"]
                })),
            )
                .into_response();
        }
        if method == Method::PUT && path.starts_with("/rest/api/2/issue/ABC-1") {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::DELETE && path.starts_with("/rest/api/2/issue/ABC-1") {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::POST && path == "/rest/api/2/issue" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "10002",
                    "key": "ABC-2",
                    "fields": {
                        "summary": "Created issue",
                        "project": {"key": "ABC", "name": "Demo"},
                        "issuetype": {"name": "Task"}
                    }
                })),
            )
                .into_response();
        }
        if method == Method::POST && path == "/rest/api/2/issue/bulk" {
            return (
                StatusCode::OK,
                Json(json!({
                    "issues": [{"id": "10003", "key": "ABC-3", "self": "https://jira.example/rest/api/2/issue/10003"}],
                    "errors": [{"failedElementNumber": 1, "message": "validation failed"}]
                })),
            )
                .into_response();
        }
        if method == Method::POST && path == "/rest/api/3/changelog/bulkfetch" {
            return (
                StatusCode::OK,
                Json(json!({
                    "issueChangeLogs": [
                        {
                            "issueId": "10001",
                            "changeHistories": [
                                {
                                    "id": "20001",
                                    "items": [{"field": "status", "fromString": "Open", "toString": "Done"}]
                                }
                            ]
                        }
                    ],
                    "nextPageToken": "next-token"
                })),
            )
                .into_response();
        }
        if method == Method::GET && path == "/rest/api/2/project?includeArchived=false" {
            return (
                StatusCode::OK,
                Json(json!([
                    {"id": "10000", "key": "ABC", "name": "Allowed"},
                    {"id": "10001", "key": "XYZ", "name": "Filtered"}
                ])),
            )
                .into_response();
        }
        if method == Method::GET && path == "/rest/api/2/project/ABC/versions" {
            return (
                StatusCode::OK,
                Json(json!([
                    {"id": "1", "name": "v1"},
                    {"name": "unnumbered"}
                ])),
            )
                .into_response();
        }
        if method == Method::GET && path == "/rest/api/2/project/ABC/components" {
            return (
                StatusCode::OK,
                Json(json!([
                    {"id": "10", "name": "API"},
                    {}
                ])),
            )
                .into_response();
        }
        if method == Method::POST && path == "/rest/api/2/version" {
            if parsed_body["name"] == json!("bad") {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"errorMessages": ["bad version"]})),
                )
                    .into_response();
            }
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "20000",
                    "name": parsed_body["name"],
                    "project": parsed_body["project"],
                    "released": parsed_body.get("released").cloned().unwrap_or(Value::Bool(false))
                })),
            )
                .into_response();
        }
        if method == Method::GET && path == "/rest/api/2/user?username=ada" {
            return (
                StatusCode::OK,
                Json(json!({
                    "accountId": "account-1",
                    "name": "ada",
                    "displayName": "Ada Lovelace",
                    "active": true
                })),
            )
                .into_response();
        }
        if method == Method::GET && path == "/rest/api/2/user?accountId=account-1" {
            return (
                StatusCode::OK,
                Json(json!({
                    "accountId": "account-1",
                    "displayName": "Ada Lovelace",
                    "active": true
                })),
            )
                .into_response();
        }
        if method == Method::GET && path == "/rest/api/2/issueLinkType" {
            return (
                StatusCode::OK,
                Json(json!({
                    "issueLinkTypes": [
                        {
                            "id": "10000",
                            "name": "Blocks",
                            "inward": "is blocked by",
                            "outward": "blocks"
                        },
                        {
                            "id": "10001",
                            "name": "Relates"
                        }
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::POST && path == "/rest/api/2/issueLink" {
            return (
                StatusCode::CREATED,
                Json(json!({"id": "200", "type": parsed_body["type"]})),
            )
                .into_response();
        }
        if method == Method::POST && path == "/rest/api/2/issue/ABC-1/remotelink" {
            return (
                StatusCode::CREATED,
                Json(json!({"id": "300", "object": parsed_body["object"]})),
            )
                .into_response();
        }
        if method == Method::DELETE && path == "/rest/api/2/issueLink/200" {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::GET
            && path.starts_with("/rest/agile/1.0/board?")
            && path.contains("projectKeyOrId=NOAGILE")
        {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"errorMessages": ["Jira Software is not available"]})),
            )
                .into_response();
        }
        if method == Method::GET && path.starts_with("/rest/agile/1.0/board?") {
            return (
                StatusCode::OK,
                Json(json!({
                    "startAt": 0,
                    "maxResults": 2,
                    "total": 1,
                    "isLast": true,
                    "values": [
                        {"id": 1, "name": "Alpha board", "type": "scrum"}
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::GET && path.starts_with("/rest/agile/1.0/board/1/issue?") {
            return (
                StatusCode::OK,
                Json(json!({
                    "startAt": 0,
                    "maxResults": 2,
                    "total": 1,
                    "issues": [
                        {"id": "10001", "key": "ABC-1", "fields": {"summary": "Sprint issue"}}
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::GET && path.starts_with("/rest/agile/1.0/board/1/sprint?") {
            return (
                StatusCode::OK,
                Json(json!({
                    "startAt": 0,
                    "maxResults": 2,
                    "total": 1,
                    "isLast": true,
                    "values": [
                        {"id": 2, "name": "Sprint 2", "state": "active"}
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::GET && path.starts_with("/rest/agile/1.0/sprint/2/issue?") {
            return (
                StatusCode::OK,
                Json(json!({
                    "startAt": 0,
                    "maxResults": 2,
                    "total": 1,
                    "issues": [
                        {"id": "10001", "key": "ABC-1", "fields": {"summary": "Sprint issue"}}
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::POST && path == "/rest/agile/1.0/sprint" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": 2,
                    "name": parsed_body["name"],
                    "originBoardId": parsed_body["originBoardId"],
                    "state": "future"
                })),
            )
                .into_response();
        }
        if method == Method::PUT && path == "/rest/agile/1.0/sprint/2" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": 2,
                    "name": parsed_body["name"],
                    "state": parsed_body["state"],
                    "goal": parsed_body["goal"]
                })),
            )
                .into_response();
        }
        if method == Method::POST && path == "/rest/agile/1.0/sprint/2/issue" {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::GET && path_only.starts_with("/jsm-down/rest/servicedeskapi") {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"errorMessages": ["Jira Service Management is not available"]})),
            )
                .into_response();
        }
        if method == Method::GET && path == "/rest/servicedeskapi/servicedesk" {
            return (
                StatusCode::OK,
                Json(json!({
                    "size": 2,
                    "values": [
                        {"id": "4", "projectKey": "ABC", "serviceDeskName": "Support"},
                        {"id": "5", "projectKey": "XYZ", "serviceDeskName": "Other"}
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::GET
            && path == "/rest/servicedeskapi/servicedesk/4/queue?start=0&limit=50"
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "start": 0,
                    "limit": 50,
                    "size": 1,
                    "values": [
                        {"id": "47", "name": "Open requests"}
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::GET
            && path == "/rest/servicedeskapi/servicedesk/4/queue/47/issue?start=0&limit=2"
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "start": 0,
                    "limit": 2,
                    "size": 1,
                    "values": [
                        {"id": "10001", "key": "ABC-1", "fields": {"summary": "Customer request"}}
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::GET && path == "/jira/forms/cloud/cloud-123/issue/ABC-1/form" {
            return (
                StatusCode::OK,
                Json(json!({
                    "forms": [
                        {
                            "id": "form-1",
                            "name": "Request form",
                            "state": {"status": "o"},
                            "submitted": false
                        }
                    ]
                })),
            )
                .into_response();
        }
        if method == Method::GET && path == "/jira/forms/cloud/cloud-123/issue/ABC-1/form/form-1" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "form-1",
                    "name": "Request form",
                    "state": {"status": "o"},
                    "design": {"content": []},
                    "answers": {"q1": {"text": "Existing"}}
                })),
            )
                .into_response();
        }
        if method == Method::PUT && path == "/jira/forms/cloud/cloud-123/issue/ABC-1/form/form-1" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "form-1",
                    "updated": true,
                    "answers": parsed_body["answers"]
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only.starts_with("/jira/forms/cloud/forms-down/") {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"errorMessages": ["Jira Forms is not available"]})),
            )
                .into_response();
        }
        if method == Method::GET && path_only.starts_with("/dev-down/rest/dev-status") {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"errorMessages": ["Jira development status is not available"]})),
            )
                .into_response();
        }
        if method == Method::GET && path.starts_with("/rest/dev-status/1.0/issue/detail?") {
            return (
                StatusCode::OK,
                Json(json!({
                    "detail": [
                        {
                            "applicationType": "github",
                            "dataType": "pullrequest",
                            "branches": [{"name": "main"}],
                            "pullRequests": [{"id": "pr-1", "name": "Fix bug"}],
                            "commits": [{"id": "commit-1", "displayId": "abc123"}]
                        }
                    ]
                })),
            )
                .into_response();
        }

        (
            StatusCode::NOT_FOUND,
            Json(json!({"errorMessages": ["missing"]})),
        )
            .into_response()
    }

    async fn mock_confluence_handler(
        State(state): State<MockConfluenceState>,
        method: Method,
        headers: HeaderMap,
        uri: axum::http::Uri,
        body: Bytes,
    ) -> Response {
        let parsed_body = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
        };
        let path = uri
            .path_and_query()
            .map(ToString::to_string)
            .unwrap_or_else(|| uri.path().to_string());
        state.requests.lock().await.push(RecordedRequest {
            method: method.clone(),
            path: path.clone(),
            body: parsed_body.clone(),
        });

        let authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        let expected_pat_header = format!("Bearer {}", "test-pat-value");
        if authorization != Some(expected_pat_header.as_str())
            && !authorization.is_some_and(|value| value.starts_with("Basic "))
        {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"errorMessages": ["auth"]})),
            )
                .into_response();
        }

        let path_only = uri.path();
        if method == Method::GET && path_only == "/rest/api/content/123" {
            if let Some(version) = query_value(&path, "version") {
                let (title, storage_value) = match version.as_str() {
                    "1" => ("Roadmap", "<h1>Roadmap</h1><p>Hello team</p>"),
                    "2" => ("Roadmap", "<h1>Roadmap</h1><p>Hello team and partners</p>"),
                    _ => {
                        return (
                            StatusCode::NOT_FOUND,
                            Json(json!({"errorMessages": ["historical version not found"]})),
                        )
                            .into_response();
                    }
                };
                return (
                    StatusCode::OK,
                    Json(json!({
                        "id": "123",
                        "title": title,
                        "type": "page",
                        "status": "historical",
                        "space": {"key": "ENG", "name": "Engineering"},
                        "body": {"storage": {"value": storage_value}},
                        "version": {"number": version.parse::<u64>().unwrap()},
                        "ancestors": [{"id": "100", "title": "Home"}],
                        "_links": {"webui": "/spaces/ENG/pages/123/Roadmap"}
                    })),
                )
                    .into_response();
            }

            return (
                StatusCode::OK,
                Json(json!({
                    "id": "123",
                    "title": "Roadmap",
                    "type": "page",
                    "status": "current",
                    "space": {"key": "ENG", "name": "Engineering"},
                    "body": {"storage": {"value": "<h1>Roadmap</h1><p>Hello &amp; welcome</p>"}},
                    "version": {"number": 7, "message": "Updated"},
                    "ancestors": [{"id": "100", "title": "Home"}],
                    "metadata": {"labels": {"results": [{"name": "planning"}]}},
                    "_links": {"webui": "/spaces/ENG/pages/123/Roadmap"}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/123/child/comment" {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [
                        {
                            "id": "c-1",
                            "title": "Roadmap",
                            "type": "comment",
                            "body": {"storage": {"value": "<p>First comment</p>"}},
                            "version": {"number": 2, "by": {"displayName": "Ada"}},
                            "container": {"id": "123", "type": "page", "title": "Roadmap"},
                            "extensions": {"location": "footer"},
                            "_links": {"webui": "/spaces/ENG/pages/123?focusedCommentId=c-1"}
                        },
                        {
                            "id": "c-2",
                            "type": "comment",
                            "body": {"storage": {"value": "<p>Reply</p>"}},
                            "version": {"number": 1, "by": {"displayName": "Lin"}},
                            "container": {"id": "c-1", "type": "comment", "title": "Roadmap"}
                        }
                    ],
                    "start": 0,
                    "limit": 25,
                    "size": 2,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/empty/child/comment" {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [],
                    "start": 0,
                    "limit": 25,
                    "size": 0,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/123/label" {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [
                        {"id": "label-1", "name": "draft", "prefix": "global", "label": "draft", "type": "label"},
                        {"id": "label-2", "name": "team", "prefix": "my", "label": "team", "type": "label"}
                    ],
                    "start": 0,
                    "limit": 200,
                    "size": 2,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/empty-labels/label" {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [],
                    "start": 0,
                    "limit": 200,
                    "size": 0,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::POST && path_only == "/rest/api/content/123/label" {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::POST && path_only == "/rest/api/content/label-error/label" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"errorMessages": ["label failed"]})),
            )
                .into_response();
        }
        if method == Method::POST && path_only == "/rest/api/content" {
            if parsed_body["type"] == json!("comment") {
                let container_id = parsed_body["container"]["id"].as_str().unwrap_or("");
                if container_id == "comment-error" || container_id == "reply-error" {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"errorMessages": ["comment failed"]})),
                    )
                        .into_response();
                }
                let is_reply = parsed_body["container"]["type"] == json!("comment");
                let comment_id = if is_reply { "c-2" } else { "c-1" };
                let display_name = if is_reply { "Lin" } else { "Ada" };
                return (
                    StatusCode::OK,
                    Json(json!({
                        "id": comment_id,
                        "title": "Roadmap",
                        "type": "comment",
                        "body": parsed_body["body"],
                        "version": {"number": 1, "by": {"displayName": display_name}},
                        "container": parsed_body["container"],
                        "extensions": {"location": "footer"},
                        "_links": {"webui": "/spaces/ENG/pages/123?focusedCommentId=c-1"}
                    })),
                )
                    .into_response();
            }

            return (
                StatusCode::OK,
                Json(json!({
                    "id": "900",
                    "title": parsed_body["title"],
                    "type": "page",
                    "status": "current",
                    "space": parsed_body["space"],
                    "body": parsed_body["body"],
                    "version": {"number": 1},
                    "ancestors": parsed_body.get("ancestors").cloned().unwrap_or(Value::Array(vec![]))
                })),
            )
                .into_response();
        }
        if method == Method::PUT
            && (path_only == "/rest/api/content/900/property/emoji-title-published"
                || path_only == "/rest/api/content/123/property/emoji-title-published")
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "key": "emoji-title-published",
                    "value": parsed_body["value"]
                })),
            )
                .into_response();
        }
        if method == Method::PUT && path_only == "/rest/api/content/123" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "123",
                    "title": parsed_body["title"],
                    "type": "page",
                    "status": "current",
                    "space": parsed_body["space"],
                    "body": parsed_body["body"],
                    "version": parsed_body["version"],
                    "ancestors": parsed_body.get("ancestors").cloned().unwrap_or(Value::Array(vec![]))
                })),
            )
                .into_response();
        }
        if method == Method::PUT && path_only == "/rest/api/content/123/move/above/999" {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::DELETE && path_only == "/rest/api/content/123" {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::DELETE && path_only == "/rest/api/content/delete-error" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"errorMessages": ["delete failed"]})),
            )
                .into_response();
        }
        if method == Method::PUT && path_only == "/rest/api/content/123/child/attachment" {
            if headers
                .get("x-atlassian-token")
                .and_then(|value| value.to_str().ok())
                != Some("nocheck")
            {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"errorMessages": ["missing attachment upload token"]})),
                )
                    .into_response();
            }

            let body_text = parsed_body.as_str().unwrap_or("");
            let title = if body_text.contains("batch-1.txt") {
                "batch-1.txt"
            } else if body_text.contains("upload.txt") {
                "upload.txt"
            } else {
                "uploaded.bin"
            };
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [{
                        "id": format!("uploaded-{title}"),
                        "type": "attachment",
                        "title": title,
                        "status": "current",
                        "extensions": {"mediaType": "application/octet-stream", "fileSize": 5},
                        "_links": {"download": format!("/download/attachments/uploaded/{title}")}
                    }]
                })),
            )
                .into_response();
        }
        if method == Method::PUT && path_only == "/rest/api/content/upload-error/child/attachment" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"errorMessages": ["upload failed"]})),
            )
                .into_response();
        }
        if method == Method::DELETE && path_only == "/rest/api/content/att-1" {
            return StatusCode::NO_CONTENT.into_response();
        }
        if method == Method::DELETE && path_only == "/rest/api/content/att-delete-error" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"errorMessages": ["delete attachment failed"]})),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/missing" {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"errorMessages": ["page not found"]})),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/att-1" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "att-1",
                    "type": "attachment",
                    "title": "file.png",
                    "status": "current",
                    "extensions": {"mediaType": "image/png", "fileSize": 11},
                    "_links": {"download": "/download/attachments/att-1/file.png?token=secret"}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/att-no-url" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "att-no-url",
                    "type": "attachment",
                    "title": "missing.bin",
                    "extensions": {"mediaType": "application/octet-stream", "fileSize": 12}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/att-large" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "att-large",
                    "type": "attachment",
                    "title": "large.bin",
                    "extensions": {
                        "mediaType": "application/octet-stream",
                        "fileSize": crate::confluence::client::DEFAULT_ATTACHMENT_MAX_BYTES + 1
                    },
                    "_links": {"download": "/download/attachments/att-large/large.bin"}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/att-stream-large" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "att-stream-large",
                    "type": "attachment",
                    "title": "large-stream.bin",
                    "extensions": {"mediaType": "application/octet-stream"},
                    "_links": {"download": "/download/attachments/att-stream-large/large.bin"}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/att-cross" {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": "att-cross",
                    "type": "attachment",
                    "title": "cross.png",
                    "extensions": {"mediaType": "image/png", "fileSize": 11},
                    "_links": {"download": "https://other.example/download/cross.png?token=secret"}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/download/attachments/att-1/file.png" {
            return Bytes::from_static(b"image-bytes").into_response();
        }
        if method == Method::GET && path_only == "/download/attachments/att-octet-image/photo.jpg" {
            return Bytes::from_static(b"photo-bytes").into_response();
        }
        if method == Method::GET && path_only == "/download/attachments/att-stream-large/large.bin"
        {
            let bytes =
                vec![b'x'; crate::confluence::client::DEFAULT_ATTACHMENT_MAX_BYTES as usize + 1];
            return bytes.into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/123/child/page" {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [{
                        "id": "201",
                        "title": "Child page",
                        "type": "page",
                        "status": "current",
                        "space": {"key": "ENG", "name": "Engineering"},
                        "body": {"storage": {"value": "<p>Child body</p>"}},
                        "version": {"number": 1}
                    }],
                    "start": 0,
                    "limit": 2,
                    "size": 1
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/123/child/folder" {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [{
                        "id": "301",
                        "title": "Folder",
                        "type": "folder",
                        "status": "current",
                        "space": {"key": "ENG", "name": "Engineering"}
                    }],
                    "start": 0,
                    "limit": 2,
                    "size": 1
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/123/child/attachment" {
            return (
                StatusCode::OK,
                Json(json!({
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
                            "metadata": {"mediaType": "text/plain", "fileSize": 12},
                            "_links": {"download": "/download/attachments/att-2/notes.txt"}
                        }
                    ],
                    "start": query_value(&path, "start").and_then(|value| value.parse::<u64>().ok()).unwrap_or(0),
                    "limit": query_value(&path, "limit").and_then(|value| value.parse::<u64>().ok()).unwrap_or(50),
                    "size": 2,
                    "_links": {"next": "/rest/api/content/123/child/attachment?start=2"}
                })),
            )
                .into_response();
        }
        if method == Method::GET
            && path_only == "/rest/api/content/empty-attachments/child/attachment"
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [],
                    "start": 0,
                    "limit": 50,
                    "size": 0,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::GET
            && path_only == "/rest/api/content/missing-attachment-fields/child/attachment"
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [{"id": "att-min"}],
                    "start": 0,
                    "limit": 50,
                    "size": 1,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/download-batch/child/attachment"
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [
                        {
                            "id": "att-1",
                            "type": "attachment",
                            "title": "file.png",
                            "extensions": {"mediaType": "image/png", "fileSize": 11},
                            "_links": {"download": "/download/attachments/att-1/file.png?token=secret"}
                        },
                        {
                            "id": "att-no-url",
                            "type": "attachment",
                            "title": "missing.bin",
                            "extensions": {"mediaType": "application/octet-stream", "fileSize": 12}
                        },
                        {
                            "id": "att-large",
                            "type": "attachment",
                            "title": "large.bin",
                            "extensions": {
                                "mediaType": "application/octet-stream",
                                "fileSize": crate::confluence::client::DEFAULT_ATTACHMENT_MAX_BYTES + 1
                            },
                            "_links": {"download": "/download/attachments/att-large/large.bin"}
                        }
                    ],
                    "start": 0,
                    "limit": 100,
                    "size": 3,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content/images/child/attachment" {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [
                        {
                            "id": "att-1",
                            "type": "attachment",
                            "title": "file.png",
                            "extensions": {"mediaType": "image/png", "fileSize": 11},
                            "_links": {"download": "/download/attachments/att-1/file.png?token=secret"}
                        },
                        {
                            "id": "att-octet-image",
                            "type": "attachment",
                            "title": "photo.jpg",
                            "extensions": {"mediaType": "application/octet-stream", "fileSize": 11},
                            "_links": {"download": "/download/attachments/att-octet-image/photo.jpg"}
                        },
                        {
                            "id": "att-2",
                            "type": "attachment",
                            "title": "notes.txt",
                            "metadata": {"mediaType": "text/plain", "fileSize": 12},
                            "_links": {"download": "/download/attachments/att-2/notes.txt"}
                        }
                    ],
                    "start": 0,
                    "limit": 100,
                    "size": 3,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/content" {
            if query_value(&path, "type").as_deref() == Some("page") {
                let limit = query_value(&path, "limit");
                if limit.as_deref() == Some("1") {
                    return (
                        StatusCode::OK,
                        Json(json!({
                            "results": [{
                                "id": "100",
                                "title": "Home",
                                "type": "page",
                                "ancestors": [],
                                "extensions": {"position": 0}
                            }],
                            "start": 0,
                            "limit": 1,
                            "size": 1,
                            "_links": {"next": "/rest/api/content?start=1"}
                        })),
                    )
                        .into_response();
                }

                return (
                    StatusCode::OK,
                    Json(json!({
                        "results": [
                            {
                                "id": "200",
                                "title": "Child",
                                "type": "page",
                                "ancestors": [{"id": "100", "title": "Home"}],
                                "extensions": {"position": 1}
                            },
                            {
                                "id": "100",
                                "title": "Home",
                                "type": "page",
                                "ancestors": [],
                                "extensions": {"position": 0}
                            }
                        ],
                        "start": 0,
                        "limit": 2,
                        "size": 2,
                        "_links": {}
                    })),
                )
                    .into_response();
            }

            let title = query_value(&path, "title");
            let space_key = query_value(&path, "spaceKey");
            if title.as_deref() == Some("Roadmap") && space_key.as_deref() == Some("ENG") {
                return (
                    StatusCode::OK,
                    Json(json!({
                        "results": [{
                            "id": "123",
                            "title": "Roadmap",
                            "type": "page",
                            "status": "current",
                            "space": {"key": "ENG", "name": "Engineering"},
                            "body": {"storage": {"value": "<p>Raw storage</p>"}},
                            "version": {"number": 7}
                        }],
                        "start": 0,
                        "limit": 1,
                        "size": 1
                    })),
                )
                    .into_response();
            }

            return (
                StatusCode::OK,
                Json(json!({"results": [], "start": 0, "limit": 1, "size": 0})),
            )
                .into_response();
        }

        if method == Method::GET && path.starts_with("/rest/api/content/search?") {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [
                        {
                            "id": "123",
                            "title": "Roadmap",
                            "excerpt": "<p>Planning</p>",
                            "content": {
                                "id": "123",
                                "title": "Roadmap",
                                "type": "page",
                                "space": {"key": "ENG", "name": "Engineering"}
                            },
                            "space": {"key": "ENG", "name": "Engineering"}
                        }
                    ],
                    "start": 0,
                    "limit": 10,
                    "size": 1
                })),
            )
                .into_response();
        }
        if method == Method::GET && path.starts_with("/rest/api/search/user?") {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [{
                        "title": "Ada Lovelace",
                        "entityType": "user",
                        "score": 0.9,
                        "user": {
                            "accountId": "abc",
                            "displayName": "Ada Lovelace",
                            "email": "ada@example.com",
                            "accountStatus": "active",
                            "profilePicture": {"path": "/avatar/ada.png"}
                        }
                    }],
                    "start": 0,
                    "limit": 5,
                    "totalSize": 1,
                    "cqlQuery": query_value(&path, "cql").unwrap_or_default(),
                    "searchDuration": 7
                })),
            )
                .into_response();
        }
        if method == Method::GET
            && (path_only == "/rest/api/group/confluence-users/member"
                || path_only == "/rest/api/group/confluence%20users/member")
        {
            return (
                StatusCode::OK,
                Json(json!({
                    "results": [
                        {"username": "ada", "displayName": "Ada Lovelace", "email": "ada@example.com"},
                        {"username": "grace", "displayName": "Grace Hopper", "email": "grace@example.com"}
                    ],
                    "start": 0,
                    "limit": 200,
                    "size": 2,
                    "_links": {}
                })),
            )
                .into_response();
        }
        if method == Method::GET && path_only == "/rest/api/analytics/content/123/views" {
            return (
                StatusCode::OK,
                Json(json!({
                    "count": 42,
                    "lastSeen": "2026-06-04T12:00:00Z",
                    "uniqueViewers": 7
                })),
            )
                .into_response();
        }

        (
            StatusCode::NOT_FOUND,
            Json(json!({"errorMessages": ["missing"]})),
        )
            .into_response()
    }

    async fn mock_jira_server() -> (String, Arc<Mutex<Vec<RecordedRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .fallback(any(mock_jira_handler))
            .with_state(MockJiraState {
                requests: requests.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{address}"), requests)
    }

    async fn mock_confluence_server() -> (String, Arc<Mutex<Vec<RecordedRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .fallback(any(mock_confluence_handler))
            .with_state(MockConfluenceState {
                requests: requests.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{address}"), requests)
    }

    #[test]
    fn server_info_advertises_tools() {
        let info = AtlassianMcpServer::default().get_info();

        assert_eq!(info.server_info.name, SERVER_NAME);
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.prompts.is_none());
        assert!(info.capabilities.resources.is_none());
    }

    #[test]
    fn tool_metadata_is_generated() {
        assert_eq!(
            AtlassianMcpServer::migration_status_tool_attr().name,
            MIGRATION_STATUS_TOOL_NAME
        );
    }

    #[test]
    fn migration_status_reports_stage_scope() {
        let server = AtlassianMcpServer::default();
        let status = server.migration_status();

        assert!(status.contains("Stage 2 Jira core migration is complete"));
        assert!(status.contains("Jira config/auth/client/models/tool handlers are implemented"));
    }

    #[test]
    fn server_info_uses_app_context() {
        let config = RuntimeConfig {
            read_only: true,
            ..RuntimeConfig::default()
        };
        let server = AtlassianMcpServer::new(Arc::new(AppContext::from_config(&config)));
        let info = server.get_info();
        let instructions = info.instructions.unwrap_or_default();

        assert!(instructions.contains("read-only mode"));
        assert!(instructions.contains("Jira core tools are available"));
    }

    #[test]
    fn tool_discovery_uses_registry_and_keeps_migration_status_visible_by_default() {
        let server = AtlassianMcpServer::default();

        assert_eq!(
            current_tool_names(&server),
            vec![MIGRATION_STATUS_TOOL_NAME.to_string()]
        );
        assert!(server.get_tool(MIGRATION_STATUS_TOOL_NAME).is_some());
    }

    #[test]
    fn tool_discovery_lists_jira_default_tools_when_configured() {
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            ..runtime_config()
        });
        let names = current_tool_names(&server);

        for name in expected_stage_two_default_tools() {
            assert!(names.contains(&name), "{name} should be visible by default");
        }
        assert!(server.get_tool(tools::JIRA_GET_ISSUE_TOOL_NAME).is_some());
    }

    #[tokio::test]
    async fn jira_get_issue_handler_returns_structured_content_from_mock_rest() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_get_issue(Parameters(tools::JiraGetIssueArgs {
                issue_key: "ABC-1".to_string(),
                fields: Some(json!(["summary"])),
                expand: None,
                comment_limit: None,
                properties: None,
                update_history: None,
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["key"],
            json!("ABC-1")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::GET);
        assert!(requests[0].path.starts_with("/rest/api/2/issue/ABC-1"));
    }

    #[tokio::test]
    async fn jira_create_issue_handler_posts_expected_payload_to_mock_rest() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_create_issue(Parameters(tools::JiraCreateIssueArgs {
                project_key: "ABC".to_string(),
                summary: "Created issue".to_string(),
                issue_type: "Task".to_string(),
                assignee: None,
                description: Some("Plain description".to_string()),
                components: Some(json!("Frontend, API")),
                additional_fields: Some(json!({"priority": {"name": "High"}})),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["data"]["key"],
            json!("ABC-2")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/api/2/issue");
        assert_eq!(requests[0].body["fields"]["project"]["key"], json!("ABC"));
        assert_eq!(
            requests[0].body["fields"]["summary"],
            json!("Created issue")
        );
        assert_eq!(
            requests[0].body["fields"]["issuetype"]["name"],
            json!("Task")
        );
        assert_eq!(
            requests[0].body["fields"]["description"],
            json!("Plain description")
        );
        assert_eq!(
            requests[0].body["fields"]["components"],
            json!([{"name": "Frontend"}, {"name": "API"}])
        );
        assert_eq!(
            requests[0].body["fields"]["priority"]["name"],
            json!("High")
        );
    }

    #[tokio::test]
    async fn jira_create_issue_handler_rejects_invalid_additional_fields_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .jira_create_issue(Parameters(tools::JiraCreateIssueArgs {
                project_key: "ABC".to_string(),
                summary: "Created issue".to_string(),
                issue_type: "Task".to_string(),
                assignee: None,
                description: None,
                components: None,
                additional_fields: Some(json!("[]")),
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(
            error
                .message
                .contains("additional_fields must be a JSON object")
        );
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_batch_create_issues_handler_posts_bulk_payload_to_mock_rest() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_batch_create_issues(Parameters(tools::JiraBatchCreateIssuesArgs {
                issues: json!([
                    {
                        "project_key": "ABC",
                        "summary": "Batch one",
                        "issue_type": "Task",
                        "description": "First description",
                        "components": ["Frontend"]
                    },
                    {
                        "project_key": "ABC",
                        "summary": "Batch two",
                        "issue_type": "Bug",
                        "priority": {"name": "High"}
                    }
                ]),
                validate_only: Some(false),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["data"]["issues"][0]["key"],
            json!("ABC-3")
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["data"]["errors"][0]["failedElementNumber"],
            json!(1)
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/api/2/issue/bulk");
        assert_eq!(requests[0].body["validateOnly"], json!(false));
        assert_eq!(
            requests[0].body["issueUpdates"][0]["fields"]["summary"],
            json!("Batch one")
        );
        assert_eq!(
            requests[0].body["issueUpdates"][0]["fields"]["components"],
            json!([{"name": "Frontend"}])
        );
        assert_eq!(
            requests[0].body["issueUpdates"][1]["fields"]["priority"]["name"],
            json!("High")
        );
    }

    #[tokio::test]
    async fn jira_batch_create_issues_handler_rejects_invalid_issue_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .jira_batch_create_issues(Parameters(tools::JiraBatchCreateIssuesArgs {
                issues: json!([{
                    "project_key": "ABC",
                    "issue_type": "Task"
                }]),
                validate_only: Some(false),
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(error.message.contains("summary is required"));
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_batch_get_changelogs_handler_posts_cloud_payload_to_mock_rest() {
        let (base_url, requests) = mock_jira_server().await;
        let mut jira = jira_config_with_base_url(base_url);
        jira.deployment = JiraDeployment::Cloud;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira),
            ..runtime_config()
        });
        let result = server
            .jira_batch_get_changelogs(Parameters(tools::JiraBatchGetChangelogsArgs {
                issue_ids_or_keys: json!(["ABC-1", "ABC-2"]),
                fields: Some(json!("status,assignee")),
                limit: Some(25),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["issueChangeLogs"][0]["issueId"],
            json!("10001")
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["nextPageToken"],
            json!("next-token")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/api/3/changelog/bulkfetch");
        assert_eq!(
            requests[0].body["issueIdsOrKeys"],
            json!(["ABC-1", "ABC-2"])
        );
        assert_eq!(requests[0].body["fieldIds"], json!(["status", "assignee"]));
        assert_eq!(requests[0].body["maxResults"], json!(25));
    }

    #[tokio::test]
    async fn jira_batch_get_changelogs_handler_returns_safe_server_dc_unsupported_result() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_batch_get_changelogs(Parameters(tools::JiraBatchGetChangelogsArgs {
                issue_ids_or_keys: json!("ABC-1"),
                fields: None,
                limit: None,
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["success"],
            json!(false)
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["product_dependency"]["available"],
            json!(false)
        );
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_update_issue_handler_puts_expected_payload_and_handles_no_content() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_update_issue(Parameters(tools::JiraUpdateIssueArgs {
                issue_key: "ABC-1".to_string(),
                fields: json!({
                    "summary": "Updated",
                    "description": "Updated description"
                }),
                additional_fields: Some(json!({"priority": {"name": "High"}})),
                components: Some(json!("Frontend, API")),
                notify_users: Some(false),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["data"],
            Value::Null
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::PUT);
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/ABC-1?notifyUsers=false"
        );
        assert_eq!(requests[0].body["fields"]["summary"], json!("Updated"));
        assert_eq!(
            requests[0].body["fields"]["description"],
            json!("Updated description")
        );
        assert_eq!(
            requests[0].body["fields"]["priority"]["name"],
            json!("High")
        );
        assert_eq!(
            requests[0].body["fields"]["components"],
            json!([{"name": "Frontend"}, {"name": "API"}])
        );
    }

    #[tokio::test]
    async fn jira_update_issue_handler_rejects_attachments_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .jira_update_issue(Parameters(tools::JiraUpdateIssueArgs {
                issue_key: "ABC-1".to_string(),
                fields: json!({"attachments": ["/tmp/file.txt"]}),
                additional_fields: None,
                components: None,
                notify_users: None,
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(
            error
                .message
                .contains("attachments is not supported by jira_update_issue")
        );
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_delete_issue_handler_sends_delete_subtasks_query_and_handles_no_content() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_delete_issue(Parameters(tools::JiraDeleteIssueArgs {
                issue_key: "ABC-1".to_string(),
                delete_subtasks: Some(true),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["data"],
            Value::Null
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::DELETE);
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/ABC-1?deleteSubtasks=true"
        );
    }

    #[tokio::test]
    async fn jira_project_read_handlers_use_project_filter_and_tolerate_sparse_values() {
        let (base_url, requests) = mock_jira_server().await;
        let mut jira = jira_config_with_base_url(base_url);
        jira.projects_filter = BTreeSet::from(["ABC".to_string()]);
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira),
            ..runtime_config()
        });

        let projects = server
            .jira_get_all_projects(Parameters(tools::JiraGetAllProjectsArgs {
                include_archived: Some(false),
            }))
            .await
            .unwrap();
        let versions = server
            .jira_get_project_versions(Parameters(tools::JiraGetProjectVersionsArgs {
                project_key: "ABC".to_string(),
            }))
            .await
            .unwrap();
        let components = server
            .jira_get_project_components(Parameters(tools::JiraGetProjectComponentsArgs {
                project_key: "ABC".to_string(),
            }))
            .await
            .unwrap();
        let forbidden_versions = server
            .jira_get_project_versions(Parameters(tools::JiraGetProjectVersionsArgs {
                project_key: "XYZ".to_string(),
            }))
            .await
            .unwrap_err();
        let forbidden_components = server
            .jira_get_project_components(Parameters(tools::JiraGetProjectComponentsArgs {
                project_key: "XYZ".to_string(),
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(
            projects.structured_content.as_ref().unwrap()[0]["key"],
            json!("ABC")
        );
        assert_eq!(
            projects
                .structured_content
                .as_ref()
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            versions.structured_content.as_ref().unwrap()[0]["name"],
            json!("v1")
        );
        assert_eq!(
            components.structured_content.as_ref().unwrap()[1],
            json!({})
        );
        assert_eq!(
            requests[0].path,
            "/rest/api/2/project?includeArchived=false"
        );
        assert_eq!(requests[1].path, "/rest/api/2/project/ABC/versions");
        assert_eq!(requests[2].path, "/rest/api/2/project/ABC/components");
        assert!(
            forbidden_versions
                .message
                .contains("outside the configured Jira project filter")
        );
        assert!(
            forbidden_components
                .message
                .contains("outside the configured Jira project filter")
        );
        assert_eq!(requests.len(), 3);
    }

    #[tokio::test]
    async fn jira_create_version_handler_posts_expected_payload_to_mock_rest() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_create_version(Parameters(tools::JiraCreateVersionArgs {
                project_key: "ABC".to_string(),
                name: "v1".to_string(),
                start_date: Some("2026-01-01".to_string()),
                release_date: Some("2026-02-01".to_string()),
                description: Some("First release".to_string()),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["name"],
            json!("v1")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/api/2/version");
        assert_eq!(requests[0].body["project"], json!("ABC"));
        assert_eq!(requests[0].body["name"], json!("v1"));
        assert_eq!(requests[0].body["startDate"], json!("2026-01-01"));
        assert_eq!(requests[0].body["releaseDate"], json!("2026-02-01"));
        assert_eq!(requests[0].body["description"], json!("First release"));
    }

    #[tokio::test]
    async fn jira_batch_create_versions_handler_returns_success_and_error_partitions() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_batch_create_versions(Parameters(tools::JiraBatchCreateVersionsArgs {
                project_key: "ABC".to_string(),
                versions: json!([
                    {"name": "v2", "released": true},
                    {"name": "bad"}
                ]),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["versions"][0]["success"],
            json!(true)
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["versions"][1]["success"],
            json!(false)
        );
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/rest/api/2/version");
        assert_eq!(requests[0].body["project"], json!("ABC"));
        assert_eq!(requests[0].body["released"], json!(true));
        assert_eq!(requests[1].body["name"], json!("bad"));
    }

    #[tokio::test]
    async fn jira_get_user_profile_handler_allows_absent_email_privacy_field() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_get_user_profile(Parameters(tools::JiraGetUserProfileArgs {
                user_identifier: "ada".to_string(),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["displayName"],
            json!("Ada Lovelace")
        );
        assert!(
            result.structured_content.as_ref().unwrap()["emailAddress"].is_null(),
            "emailAddress should not be required in privacy-filtered responses"
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/rest/api/2/user?username=ada");
    }

    #[tokio::test]
    async fn jira_watcher_handlers_read_add_and_remove_watchers() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let watchers = server
            .jira_get_issue_watchers(Parameters(tools::JiraGetIssueWatchersArgs {
                issue_key: "ABC-1".to_string(),
            }))
            .await
            .unwrap();
        let add = server
            .jira_add_watcher(Parameters(tools::JiraAddWatcherArgs {
                issue_key: "ABC-1".to_string(),
                user_identifier: "ada".to_string(),
            }))
            .await
            .unwrap();
        let remove = server
            .jira_remove_watcher(Parameters(tools::JiraRemoveWatcherArgs {
                issue_key: "ABC-1".to_string(),
                user_identifier: "ada".to_string(),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            watchers.structured_content.as_ref().unwrap()["watchCount"],
            json!(1)
        );
        assert_eq!(
            watchers.structured_content.as_ref().unwrap()["watchers"][0]["displayName"],
            json!("Ada Lovelace")
        );
        assert_eq!(
            add.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(
            remove.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(requests[0].path, "/rest/api/2/issue/ABC-1/watchers");
        assert_eq!(requests[1].method, Method::POST);
        assert_eq!(requests[1].body, json!("ada"));
        assert_eq!(requests[2].method, Method::DELETE);
        assert_eq!(
            requests[2].path,
            "/rest/api/2/issue/ABC-1/watchers?username=ada"
        );
    }

    #[tokio::test]
    async fn jira_get_worklog_handler_sends_pagination_and_tolerates_missing_optional_fields() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_get_worklog(Parameters(tools::JiraGetWorklogArgs {
                issue_key: "ABC-1".to_string(),
                start_at: Some(0),
                limit: Some(10),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["total"],
            json!(2)
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["worklogs"][1]["author"],
            Value::Null
        );
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/ABC-1/worklog?startAt=0&maxResults=10"
        );
    }

    #[tokio::test]
    async fn jira_add_worklog_handler_posts_body_and_estimate_query() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_add_worklog(Parameters(tools::JiraAddWorklogArgs {
                issue_key: "ABC-1".to_string(),
                time_spent: "1h".to_string(),
                started: Some("2026-01-01T00:00:00.000+0000".to_string()),
                comment: Some("Worklog note".to_string()),
                visibility: Some(json!({"type": "group", "value": "jira-users"})),
                adjust_estimate: Some("new".to_string()),
                new_estimate: Some("2h".to_string()),
                reduce_by: None,
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            result.structured_content.as_ref().unwrap()["id"],
            json!("300")
        );
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/ABC-1/worklog?adjustEstimate=new&newEstimate=2h"
        );
        assert_eq!(requests[0].body["timeSpent"], json!("1h"));
        assert_eq!(
            requests[0].body["started"],
            json!("2026-01-01T00:00:00.000+0000")
        );
        assert_eq!(requests[0].body["comment"], json!("Worklog note"));
        assert_eq!(
            requests[0].body["visibility"],
            json!({"type": "group", "value": "jira-users"})
        );
    }

    #[tokio::test]
    async fn jira_add_worklog_handler_rejects_invalid_visibility_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .jira_add_worklog(Parameters(tools::JiraAddWorklogArgs {
                issue_key: "ABC-1".to_string(),
                time_spent: "1h".to_string(),
                started: None,
                comment: None,
                visibility: Some(json!("public")),
                adjust_estimate: None,
                new_estimate: None,
                reduce_by: None,
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(error.message.contains("visibility must be a JSON object"));
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_link_type_and_epic_handlers_use_expected_payloads() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let all_link_types = server
            .jira_get_link_types(Parameters(tools::JiraGetLinkTypesArgs {
                name_filter: None,
            }))
            .await
            .unwrap();
        let link_types = server
            .jira_get_link_types(Parameters(tools::JiraGetLinkTypesArgs {
                name_filter: Some("block".to_string()),
            }))
            .await
            .unwrap();
        let epic = server
            .jira_link_to_epic(Parameters(tools::JiraLinkToEpicArgs {
                issue_key: "ABC-1".to_string(),
                epic_key: "ABC-EPIC".to_string(),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        let all_link_types = &all_link_types.structured_content.as_ref().unwrap()["issueLinkTypes"];
        assert_eq!(all_link_types.as_array().unwrap().len(), 2);
        assert_eq!(all_link_types[1]["name"], json!("Relates"));
        assert!(all_link_types[1]["inward"].is_null());
        assert!(all_link_types[1]["outward"].is_null());
        let link_types = &link_types.structured_content.as_ref().unwrap()["issueLinkTypes"];
        assert_eq!(link_types.as_array().unwrap().len(), 1);
        assert_eq!(link_types[0]["name"], json!("Blocks"));
        assert_eq!(link_types[0]["inward"], json!("is blocked by"));
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[0].path, "/rest/api/2/issueLinkType");
        assert_eq!(requests[1].method, Method::GET);
        assert_eq!(requests[1].path, "/rest/api/2/issueLinkType");
        assert_eq!(
            epic.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(
            epic.structured_content.as_ref().unwrap()["data"],
            Value::Null
        );
        assert_eq!(requests[2].method, Method::PUT);
        assert_eq!(requests[2].path, "/rest/api/2/issue/ABC-1");
        assert_eq!(
            requests[2].body["fields"]["parent"],
            json!({"key": "ABC-EPIC"})
        );
    }

    #[tokio::test]
    async fn jira_issue_link_handlers_post_remote_and_delete_expected_payloads() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let issue_link = server
            .jira_create_issue_link(Parameters(tools::JiraCreateIssueLinkArgs {
                link_type: "Blocks".to_string(),
                inward_issue_key: "ABC-1".to_string(),
                outward_issue_key: "ABC-2".to_string(),
                comment: Some("Linking related work".to_string()),
            }))
            .await
            .unwrap();
        let remote_link = server
            .jira_create_remote_issue_link(Parameters(tools::JiraCreateRemoteIssueLinkArgs {
                issue_key: "ABC-1".to_string(),
                url: "https://example.invalid/doc".to_string(),
                title: "Design doc".to_string(),
                global_id: Some("system=https://example.invalid&id=doc-1".to_string()),
                summary: Some("Architecture notes".to_string()),
                relationship: Some("documents".to_string()),
                icon_url: Some("https://example.invalid/icon.png".to_string()),
                status: Some(json!({"resolved": false})),
            }))
            .await
            .unwrap();
        let remove = server
            .jira_remove_issue_link(Parameters(tools::JiraRemoveIssueLinkArgs {
                link_id: "200".to_string(),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            issue_link.structured_content.as_ref().unwrap()["id"],
            json!("200")
        );
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/api/2/issueLink");
        assert_eq!(requests[0].body["type"]["name"], json!("Blocks"));
        assert_eq!(requests[0].body["inwardIssue"]["key"], json!("ABC-1"));
        assert_eq!(requests[0].body["outwardIssue"]["key"], json!("ABC-2"));
        assert_eq!(
            requests[0].body["comment"]["body"],
            json!("Linking related work")
        );
        assert_eq!(
            remote_link.structured_content.as_ref().unwrap()["id"],
            json!("300")
        );
        assert_eq!(requests[1].method, Method::POST);
        assert_eq!(requests[1].path, "/rest/api/2/issue/ABC-1/remotelink");
        assert_eq!(
            requests[1].body["globalId"],
            json!("system=https://example.invalid&id=doc-1")
        );
        assert_eq!(requests[1].body["relationship"], json!("documents"));
        assert_eq!(
            requests[1].body["object"]["url"],
            json!("https://example.invalid/doc")
        );
        assert_eq!(requests[1].body["object"]["title"], json!("Design doc"));
        assert_eq!(
            requests[1].body["object"]["summary"],
            json!("Architecture notes")
        );
        assert_eq!(
            requests[1].body["object"]["icon"],
            json!({"url16x16": "https://example.invalid/icon.png", "title": "Design doc"})
        );
        assert_eq!(
            requests[1].body["object"]["status"],
            json!({"resolved": false})
        );
        assert_eq!(
            remove.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(
            remove.structured_content.as_ref().unwrap()["link_id"],
            json!("200")
        );
        assert_eq!(requests[2].method, Method::DELETE);
        assert_eq!(requests[2].path, "/rest/api/2/issueLink/200");
    }

    #[tokio::test]
    async fn jira_create_remote_issue_link_rejects_invalid_status_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .jira_create_remote_issue_link(Parameters(tools::JiraCreateRemoteIssueLinkArgs {
                issue_key: "ABC-1".to_string(),
                url: "https://example.invalid/doc".to_string(),
                title: "Design doc".to_string(),
                global_id: None,
                summary: None,
                relationship: None,
                icon_url: None,
                status: Some(json!("resolved")),
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(error.message.contains("status must be a JSON object"));
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_download_attachments_handler_returns_safe_metadata_and_content_results() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_download_attachments(Parameters(tools::JiraDownloadAttachmentsArgs {
                issue_key: "ABC-1".to_string(),
                attachment_ids: Some(json!(["1", "2"])),
                include_content: Some(true),
                max_bytes: Some(20),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["issue_key"], json!("ABC-1"));
        assert_eq!(structured["count"], json!(2));
        assert_eq!(structured["attachments"][0]["filename"], json!("file.png"));
        assert_eq!(structured["attachments"][0]["has_content_url"], json!(true));
        assert!(structured["attachments"][0].get("thumbnail").is_none());
        assert_eq!(
            structured["attachments"][0]["content"],
            json!({
                "encoding": "base64",
                "content_type": "image/png",
                "size": 11,
                "data": "aW1hZ2UtYnl0ZXM="
            })
        );
        assert_eq!(structured["attachments"][1]["filename"], json!("notes.txt"));
        let error = structured["attachments"][1]["content_error"]["message"]
            .as_str()
            .unwrap();
        assert!(error.contains("/secure/attachment/2/notes.txt?<redacted>"));
        assert!(!error.contains("token=secret"));
        assert!(!error.contains("client=abc"));
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/ABC-1?fields=attachment"
        );
        assert_eq!(
            requests[1].path,
            "/secure/attachment/1/file.png?token=secret"
        );
        assert_eq!(
            requests[2].path,
            "/secure/attachment/2/notes.txt?token=secret&client=abc"
        );
    }

    #[tokio::test]
    async fn jira_download_attachments_rejects_invalid_max_bytes_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .jira_download_attachments(Parameters(tools::JiraDownloadAttachmentsArgs {
                issue_key: "ABC-1".to_string(),
                attachment_ids: None,
                include_content: Some(true),
                max_bytes: Some(0),
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(error.message.contains("max_bytes must be positive"));
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_get_issue_images_handler_filters_non_images_and_returns_safe_content() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_get_issue_images(Parameters(tools::JiraGetIssueImagesArgs {
                issue_key: "ABC-1".to_string(),
                include_content: Some(true),
                max_bytes: Some(20),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["images_only"], json!(true));
        assert_eq!(structured["count"], json!(1));
        assert_eq!(structured["attachments"][0]["filename"], json!("file.png"));
        assert_eq!(structured["attachments"][0]["is_image"], json!(true));
        assert_eq!(
            structured["attachments"][0]["content"]["data"],
            json!("aW1hZ2UtYnl0ZXM=")
        );
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/ABC-1?fields=attachment"
        );
        assert_eq!(
            requests[1].path,
            "/secure/attachment/1/file.png?token=secret"
        );
        assert_eq!(
            requests.len(),
            2,
            "non-image attachment content is not fetched"
        );
    }

    #[tokio::test]
    async fn jira_get_issue_images_handler_returns_empty_list_when_issue_has_no_images() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_get_issue_images(Parameters(tools::JiraGetIssueImagesArgs {
                issue_key: "TXT-1".to_string(),
                include_content: Some(true),
                max_bytes: Some(20),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["images_only"], json!(true));
        assert_eq!(structured["count"], json!(0));
        assert_eq!(structured["attachments"], json!([]));
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/TXT-1?fields=attachment"
        );
        assert_eq!(requests.len(), 1, "no image content is fetched");
    }

    #[tokio::test]
    async fn jira_agile_read_handlers_send_expected_queries_and_return_pages() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let boards = server
            .jira_get_agile_boards(Parameters(tools::JiraGetAgileBoardsArgs {
                project_key: Some("ABC".to_string()),
                board_type: Some("scrum".to_string()),
                start_at: Some(0),
                limit: Some(2),
            }))
            .await
            .unwrap();
        let board_issues = server
            .jira_get_board_issues(Parameters(tools::JiraGetBoardIssuesArgs {
                board_id: 1,
                jql: Some("status = Done".to_string()),
                fields: Some(json!("summary,status")),
                start_at: Some(0),
                limit: Some(2),
            }))
            .await
            .unwrap();
        let sprints = server
            .jira_get_sprints_from_board(Parameters(tools::JiraGetSprintsFromBoardArgs {
                board_id: 1,
                state: Some(json!(["active", "future"])),
                start_at: Some(0),
                limit: Some(2),
            }))
            .await
            .unwrap();
        let sprint_issues = server
            .jira_get_sprint_issues(Parameters(tools::JiraGetSprintIssuesArgs {
                sprint_id: 2,
                fields: Some(json!(["summary", "status"])),
                start_at: Some(0),
                limit: Some(2),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            boards.structured_content.as_ref().unwrap()["values"][0]["name"],
            json!("Alpha board")
        );
        assert_eq!(
            board_issues.structured_content.as_ref().unwrap()["issues"][0]["key"],
            json!("ABC-1")
        );
        assert_eq!(
            sprints.structured_content.as_ref().unwrap()["values"][0]["state"],
            json!("active")
        );
        assert_eq!(
            sprint_issues.structured_content.as_ref().unwrap()["issues"][0]["fields"]["summary"],
            json!("Sprint issue")
        );
        assert_eq!(
            requests[0].path,
            "/rest/agile/1.0/board?projectKeyOrId=ABC&type=scrum&startAt=0&maxResults=2"
        );
        assert_eq!(
            requests[1].path,
            "/rest/agile/1.0/board/1/issue?jql=status+%3D+Done&fields=summary%2Cstatus&startAt=0&maxResults=2"
        );
        assert_eq!(
            requests[2].path,
            "/rest/agile/1.0/board/1/sprint?state=active%2Cfuture&startAt=0&maxResults=2"
        );
        assert_eq!(
            requests[3].path,
            "/rest/agile/1.0/sprint/2/issue?fields=summary%2Cstatus&startAt=0&maxResults=2"
        );
    }

    #[tokio::test]
    async fn jira_agile_boards_handler_returns_product_unavailable_when_software_missing() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .jira_get_agile_boards(Parameters(tools::JiraGetAgileBoardsArgs {
                project_key: Some("NOAGILE".to_string()),
                board_type: None,
                start_at: None,
                limit: None,
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["success"], json!(false));
        assert_eq!(
            structured["product_dependency"]["product"],
            json!("Jira Software Agile REST")
        );
        assert_eq!(structured["product_dependency"]["available"], json!(false));
        assert_eq!(
            requests[0].path,
            "/rest/agile/1.0/board?projectKeyOrId=NOAGILE"
        );
    }

    #[tokio::test]
    async fn jira_agile_write_handlers_send_expected_payloads_and_handle_no_content() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let created = server
            .jira_create_sprint(Parameters(tools::JiraCreateSprintArgs {
                name: "Sprint 2".to_string(),
                origin_board_id: 1,
                start_date: Some("2026-01-01T00:00:00.000Z".to_string()),
                end_date: Some("2026-01-14T00:00:00.000Z".to_string()),
                goal: Some("Ship scope".to_string()),
            }))
            .await
            .unwrap();
        let updated = server
            .jira_update_sprint(Parameters(tools::JiraUpdateSprintArgs {
                sprint_id: 2,
                name: Some("Sprint 2 updated".to_string()),
                state: Some("active".to_string()),
                start_date: None,
                end_date: None,
                goal: Some("Updated goal".to_string()),
            }))
            .await
            .unwrap();
        let added = server
            .jira_add_issues_to_sprint(Parameters(tools::JiraAddIssuesToSprintArgs {
                sprint_id: 2,
                issue_keys: json!("ABC-1, ABC-2"),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            created.structured_content.as_ref().unwrap()["name"],
            json!("Sprint 2")
        );
        assert_eq!(
            updated.structured_content.as_ref().unwrap()["state"],
            json!("active")
        );
        assert_eq!(added.structured_content.as_ref().unwrap(), &Value::Null);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/agile/1.0/sprint");
        assert_eq!(requests[0].body["name"], json!("Sprint 2"));
        assert_eq!(requests[0].body["originBoardId"], json!(1));
        assert_eq!(
            requests[0].body["startDate"],
            json!("2026-01-01T00:00:00.000Z")
        );
        assert_eq!(
            requests[0].body["endDate"],
            json!("2026-01-14T00:00:00.000Z")
        );
        assert_eq!(requests[0].body["goal"], json!("Ship scope"));
        assert_eq!(requests[1].method, Method::PUT);
        assert_eq!(requests[1].path, "/rest/agile/1.0/sprint/2");
        assert_eq!(requests[1].body["name"], json!("Sprint 2 updated"));
        assert_eq!(requests[1].body["state"], json!("active"));
        assert_eq!(requests[1].body["goal"], json!("Updated goal"));
        assert!(requests[1].body["startDate"].is_null());
        assert_eq!(requests[2].method, Method::POST);
        assert_eq!(requests[2].path, "/rest/agile/1.0/sprint/2/issue");
        assert_eq!(requests[2].body["issues"], json!(["ABC-1", "ABC-2"]));
    }

    #[tokio::test]
    async fn jira_update_sprint_rejects_empty_payload_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .jira_update_sprint(Parameters(tools::JiraUpdateSprintArgs {
                sprint_id: 2,
                name: None,
                state: None,
                start_date: None,
                end_date: None,
                goal: None,
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(
            error
                .message
                .contains("sprint update must contain at least one field")
        );
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_service_desk_handlers_lookup_queues_and_queue_issues() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let desk = server
            .jira_get_service_desk_for_project(Parameters(
                tools::JiraGetServiceDeskForProjectArgs {
                    project_key: "ABC".to_string(),
                },
            ))
            .await
            .unwrap();
        let queues = server
            .jira_get_service_desk_queues(Parameters(tools::JiraGetServiceDeskQueuesArgs {
                service_desk_id: "4".to_string(),
                start_at: Some(0),
                limit: Some(50),
            }))
            .await
            .unwrap();
        let issues = server
            .jira_get_queue_issues(Parameters(tools::JiraGetQueueIssuesArgs {
                service_desk_id: "4".to_string(),
                queue_id: "47".to_string(),
                start_at: Some(0),
                limit: Some(2),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            desk.structured_content.as_ref().unwrap()["service_desk"]["id"],
            json!("4")
        );
        assert_eq!(
            queues.structured_content.as_ref().unwrap()["values"][0]["name"],
            json!("Open requests")
        );
        assert_eq!(
            issues.structured_content.as_ref().unwrap()["values"][0]["key"],
            json!("ABC-1")
        );
        assert_eq!(requests[0].path, "/rest/servicedeskapi/servicedesk");
        assert_eq!(
            requests[1].path,
            "/rest/servicedeskapi/servicedesk/4/queue?start=0&limit=50"
        );
        assert_eq!(
            requests[2].path,
            "/rest/servicedeskapi/servicedesk/4/queue/47/issue?start=0&limit=2"
        );
    }

    #[tokio::test]
    async fn jira_service_desk_handler_returns_product_unavailable_when_jsm_missing() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(format!("{base_url}/jsm-down"))),
            ..runtime_config()
        });
        let result = server
            .jira_get_service_desk_for_project(Parameters(
                tools::JiraGetServiceDeskForProjectArgs {
                    project_key: "ABC".to_string(),
                },
            ))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["success"], json!(false));
        assert_eq!(
            structured["product_dependency"]["product"],
            json!("Jira Service Management")
        );
        assert_eq!(structured["product_dependency"]["available"], json!(false));
        assert_eq!(
            requests[0].path,
            "/jsm-down/rest/servicedeskapi/servicedesk"
        );
    }

    #[tokio::test]
    async fn jira_forms_read_handlers_use_cloud_id_config_and_return_forms() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            atlassian_oauth_cloud_id: Some("cloud-123".to_string()),
            ..runtime_config()
        });

        let forms = server
            .jira_get_issue_proforma_forms(Parameters(tools::JiraGetIssueProformaFormsArgs {
                issue_key: "ABC-1".to_string(),
            }))
            .await
            .unwrap();
        let details = server
            .jira_get_proforma_form_details(Parameters(tools::JiraGetProformaFormDetailsArgs {
                issue_key: "ABC-1".to_string(),
                form_id: "form-1".to_string(),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            forms.structured_content.as_ref().unwrap()["forms"][0]["id"],
            json!("form-1")
        );
        assert_eq!(
            details.structured_content.as_ref().unwrap()["answers"]["q1"]["text"],
            json!("Existing")
        );
        assert_eq!(
            requests[0].path,
            "/jira/forms/cloud/cloud-123/issue/ABC-1/form"
        );
        assert_eq!(
            requests[1].path,
            "/jira/forms/cloud/cloud-123/issue/ABC-1/form/form-1"
        );
    }

    #[tokio::test]
    async fn jira_forms_read_handlers_return_product_unavailable_when_cloud_id_missing() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        let result = server
            .jira_get_issue_proforma_forms(Parameters(tools::JiraGetIssueProformaFormsArgs {
                issue_key: "ABC-1".to_string(),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["success"], json!(false));
        assert_eq!(
            structured["product_dependency"]["product"],
            json!("Jira Forms/ProForma Cloud ID")
        );
        assert_eq!(structured["product_dependency"]["available"], json!(false));
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_forms_read_handlers_return_product_unavailable_when_forms_api_missing() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            atlassian_oauth_cloud_id: Some("forms-down".to_string()),
            ..runtime_config()
        });

        let result = server
            .jira_get_issue_proforma_forms(Parameters(tools::JiraGetIssueProformaFormsArgs {
                issue_key: "ABC-1".to_string(),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["success"], json!(false));
        assert_eq!(
            structured["product_dependency"]["product"],
            json!("Jira Forms/ProForma")
        );
        assert_eq!(structured["product_dependency"]["available"], json!(false));
        assert_eq!(
            requests[0].path,
            "/jira/forms/cloud/forms-down/issue/ABC-1/form"
        );
    }

    #[tokio::test]
    async fn jira_forms_write_handler_sends_answer_payload() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            atlassian_oauth_cloud_id: Some("cloud-123".to_string()),
            ..runtime_config()
        });

        let result = server
            .jira_update_proforma_form_answers(Parameters(
                tools::JiraUpdateProformaFormAnswersArgs {
                    issue_key: "ABC-1".to_string(),
                    form_id: "form-1".to_string(),
                    answers: json!([
                        {"questionId": "q1", "type": "TEXT", "value": "Updated"},
                        {"questionId": "q2", "type": "SELECT", "value": "Product A"},
                        {"questionId": "q3", "type": "DATE", "value": "2026-06-04"}
                    ]),
                },
            ))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["updated"], json!(true));
        assert_eq!(structured["answers"]["q2"]["choices"], json!(["Product A"]));
        assert_eq!(requests[0].method, Method::PUT);
        assert_eq!(
            requests[0].path,
            "/jira/forms/cloud/cloud-123/issue/ABC-1/form/form-1"
        );
        assert_eq!(requests[0].body["answers"]["q1"]["text"], json!("Updated"));
        assert_eq!(
            requests[0].body["answers"]["q2"]["choices"],
            json!(["Product A"])
        );
        assert_eq!(
            requests[0].body["answers"]["q3"]["date"],
            json!("2026-06-04")
        );
    }

    #[tokio::test]
    async fn jira_forms_write_handler_returns_product_unavailable_when_cloud_id_missing() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        let result = server
            .jira_update_proforma_form_answers(Parameters(
                tools::JiraUpdateProformaFormAnswersArgs {
                    issue_key: "ABC-1".to_string(),
                    form_id: "form-1".to_string(),
                    answers: json!([{"questionId": "q1", "type": "TEXT", "value": "Updated"}]),
                },
            ))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["success"], json!(false));
        assert_eq!(
            structured["product_dependency"]["product"],
            json!("Jira Forms/ProForma Cloud ID")
        );
        assert_eq!(structured["product_dependency"]["available"], json!(false));
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_forms_write_handler_rejects_invalid_answers_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            atlassian_oauth_cloud_id: Some("cloud-123".to_string()),
            ..runtime_config()
        });

        let error = server
            .jira_update_proforma_form_answers(Parameters(
                tools::JiraUpdateProformaFormAnswersArgs {
                    issue_key: "ABC-1".to_string(),
                    form_id: "form-1".to_string(),
                    answers: json!("not-answers"),
                },
            ))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(error.message.contains("answers must be a JSON array"));
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn jira_issue_dates_handler_returns_date_fields_and_flags() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        let result = server
            .jira_get_issue_dates(Parameters(tools::JiraGetIssueDatesArgs {
                issue_key: "ABC-1".to_string(),
                include_status_changes: Some(true),
                include_status_summary: Some(true),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["issue_key"], json!("ABC-1"));
        assert_eq!(structured["include_status_changes"], json!(true));
        assert_eq!(structured["include_status_summary"], json!(true));
        assert_eq!(
            structured["issue"]["fields"]["created"],
            json!("2026-01-01T00:00:00.000+0000")
        );
        assert_eq!(
            structured["issue"]["fields"]["duedate"],
            json!("2026-01-10")
        );
        assert_eq!(structured["issue"]["status"]["name"], json!("Done"));
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/ABC-1?fields=created%2Cupdated%2Cduedate%2Cresolutiondate%2Cstatus&expand=changelog"
        );
    }

    #[tokio::test]
    async fn jira_issue_dates_handler_handles_missing_date_fields() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        let result = server
            .jira_get_issue_dates(Parameters(tools::JiraGetIssueDatesArgs {
                issue_key: "TXT-1".to_string(),
                include_status_changes: None,
                include_status_summary: None,
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["issue_key"], json!("TXT-1"));
        assert_eq!(structured["include_status_changes"], json!(false));
        assert!(structured["issue"]["fields"]["created"].is_null());
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/TXT-1?fields=created%2Cupdated%2Cduedate%2Cresolutiondate%2Cstatus"
        );
    }

    #[tokio::test]
    async fn jira_issue_sla_handler_parses_mock_sla_fields_and_args() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        let result = server
            .jira_get_issue_sla(Parameters(tools::JiraGetIssueSlaArgs {
                issue_key: "ABC-1".to_string(),
                metrics: Some(json!("time_to_resolution, time_to_first_response")),
                working_hours_only: Some(true),
                include_raw_dates: Some(true),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["issue_key"], json!("ABC-1"));
        assert_eq!(
            structured["requested_metrics"],
            json!(["time_to_resolution", "time_to_first_response"])
        );
        assert_eq!(structured["working_hours_only"], json!(true));
        assert_eq!(structured["include_raw_dates"], json!(true));
        assert_eq!(structured["success"], json!(true));
        assert_eq!(structured["count"], json!(1));
        assert_eq!(
            structured["metrics"][0]["field_id"],
            json!("customfield_sla")
        );
        assert_eq!(
            structured["product_dependency"]["product"],
            json!("Jira Service Management SLA")
        );
        assert_eq!(structured["product_dependency"]["available"], json!(true));
        assert_eq!(
            requests[0].path,
            "/rest/api/2/issue/ABC-1?fields=time_to_resolution%2Ctime_to_first_response"
        );
    }

    #[tokio::test]
    async fn jira_development_handlers_return_single_and_batch_info() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        let single = server
            .jira_get_issue_development_info(Parameters(tools::JiraGetIssueDevelopmentInfoArgs {
                issue_key: "ABC-1".to_string(),
                application_type: Some("github".to_string()),
                data_type: Some("pullrequest".to_string()),
            }))
            .await
            .unwrap();
        let batch = server
            .jira_get_issues_development_info(Parameters(tools::JiraGetIssuesDevelopmentInfoArgs {
                issue_keys: json!(["10001", "10002"]),
                application_type: Some("github".to_string()),
                data_type: Some("pullrequest".to_string()),
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;

        assert_eq!(
            single.structured_content.as_ref().unwrap()["detail"][0]["dataType"],
            json!("pullrequest")
        );
        assert_eq!(
            batch.structured_content.as_ref().unwrap()["issues"][0]["issue_key"],
            json!("10001")
        );
        assert_eq!(
            batch.structured_content.as_ref().unwrap()["issues"][1]["development"]["detail"][0]["applicationType"],
            json!("github")
        );
        assert_eq!(requests[0].path, "/rest/api/2/issue/ABC-1?fields=id%2Ckey");
        assert_eq!(
            requests[1].path,
            "/rest/dev-status/1.0/issue/detail?issueId=10001&applicationType=github&dataType=pullrequest"
        );
        assert_eq!(
            requests[2].path,
            "/rest/dev-status/1.0/issue/detail?issueId=10001&applicationType=github&dataType=pullrequest"
        );
        assert_eq!(
            requests[3].path,
            "/rest/dev-status/1.0/issue/detail?issueId=10002&applicationType=github&dataType=pullrequest"
        );
    }

    #[tokio::test]
    async fn jira_development_handler_returns_product_unavailable_when_plugin_missing() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(format!("{base_url}/dev-down"))),
            ..runtime_config()
        });

        let result = server
            .jira_get_issue_development_info(Parameters(tools::JiraGetIssueDevelopmentInfoArgs {
                issue_key: "10001".to_string(),
                application_type: None,
                data_type: None,
            }))
            .await
            .unwrap();
        let requests = requests.lock().await;
        let structured = result.structured_content.as_ref().unwrap();

        assert_eq!(structured["success"], json!(false));
        assert_eq!(
            structured["product_dependency"]["product"],
            json!("Jira development/dev-status")
        );
        assert_eq!(structured["product_dependency"]["available"], json!(false));
        assert_eq!(
            requests[0].path,
            "/dev-down/rest/dev-status/1.0/issue/detail?issueId=10001"
        );
    }

    #[tokio::test]
    async fn jira_tool_handler_rejects_invalid_json_object_input_before_http() {
        let (base_url, requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .jira_transition_issue(Parameters(tools::JiraTransitionIssueArgs {
                issue_key: "ABC-1".to_string(),
                transition_id: "31".to_string(),
                fields: Some(json!("[]")),
                comment: None,
            }))
            .await
            .unwrap_err();
        let requests = requests.lock().await;

        assert!(error.message.contains("fields must be a JSON object"));
        assert!(requests.is_empty());
    }

    #[test]
    fn stage_three_handler_arg_helpers_validate_json_shapes() {
        assert!(
            parse_required_object_arg(json!("[]"), "fields")
                .unwrap_err()
                .message
                .contains("fields must be a JSON object")
        );
        assert!(
            parse_required_object_list_arg(json!([{"fields": {"summary": "ok"}}, "bad"]), "issues")
                .unwrap_err()
                .message
                .contains("issues must contain only JSON objects")
        );
        assert!(
            parse_required_string_list_arg(json!({"bad": "shape"}), "issue_keys")
                .unwrap_err()
                .message
                .contains("issue_keys must be a string or array of strings")
        );
    }

    #[test]
    fn tool_discovery_applies_toolsets_and_read_only_to_real_jira_tools() {
        let fields_only = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_fields".to_string()]),
            ..runtime_config()
        });
        let read_only = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            ..runtime_config()
        });

        assert_eq!(
            current_tool_names(&fields_only),
            vec![
                tools::JIRA_GET_FIELD_OPTIONS_TOOL_NAME.to_string(),
                tools::JIRA_SEARCH_FIELDS_TOOL_NAME.to_string(),
                MIGRATION_STATUS_TOOL_NAME.to_string(),
            ]
        );
        assert!(
            !current_tool_names(&read_only)
                .contains(&tools::JIRA_ADD_COMMENT_TOOL_NAME.to_string())
        );
        assert!(
            read_only
                .guard_registered_tool_call(tools::JIRA_TRANSITION_ISSUE_TOOL_NAME)
                .is_err()
        );
    }

    #[test]
    fn stage_three_candidate_tool_discovery_uses_registered_metadata_at_mcp_boundary() {
        let agile_only = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_agile".to_string()]),
            ..runtime_config()
        });
        let read_only = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            ..runtime_config()
        });

        assert_eq!(
            tool_names(agile_only.filtered_tools_from(stage_three_candidate_tools())),
            vec![
                tools::JIRA_ADD_ISSUES_TO_SPRINT_TOOL_NAME.to_string(),
                tools::JIRA_CREATE_SPRINT_TOOL_NAME.to_string(),
                tools::JIRA_GET_AGILE_BOARDS_TOOL_NAME.to_string(),
                tools::JIRA_GET_BOARD_ISSUES_TOOL_NAME.to_string(),
                tools::JIRA_GET_SPRINT_ISSUES_TOOL_NAME.to_string(),
                tools::JIRA_GET_SPRINTS_FROM_BOARD_TOOL_NAME.to_string(),
                tools::JIRA_UPDATE_SPRINT_TOOL_NAME.to_string(),
            ]
        );
        assert!(
            !tool_names(read_only.filtered_tools_from(stage_three_candidate_tools()))
                .contains(&tools::JIRA_CREATE_ISSUE_TOOL_NAME.to_string())
        );
        assert!(
            tool_names(read_only.filtered_tools_from(stage_three_candidate_tools()))
                .contains(&tools::JIRA_BATCH_GET_CHANGELOGS_TOOL_NAME.to_string())
        );
    }

    #[test]
    fn c4_product_dependent_tools_have_routes_and_registered_metadata() {
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            atlassian_oauth_cloud_id: Some("cloud-123".to_string()),
            ..runtime_config()
        });
        let names = current_tool_names(&server);
        let c4_tools = stage_three_c4_tool_names();

        assert_eq!(c4_tools.len(), 17);
        for name in c4_tools {
            assert!(
                tool_registry::metadata_for(name).is_some(),
                "{name} should have registered metadata"
            );
            assert!(
                server.get_tool(name).is_some(),
                "{name} should have a route"
            );
            assert!(
                names.contains(&name.to_string()),
                "{name} should be visible"
            );
        }
    }

    #[test]
    fn c4_product_dependent_toolsets_filter_to_expected_tools() {
        let cases = [
            (
                "jira_agile",
                vec![
                    tools::JIRA_GET_AGILE_BOARDS_TOOL_NAME,
                    tools::JIRA_GET_BOARD_ISSUES_TOOL_NAME,
                    tools::JIRA_GET_SPRINTS_FROM_BOARD_TOOL_NAME,
                    tools::JIRA_GET_SPRINT_ISSUES_TOOL_NAME,
                    tools::JIRA_CREATE_SPRINT_TOOL_NAME,
                    tools::JIRA_UPDATE_SPRINT_TOOL_NAME,
                    tools::JIRA_ADD_ISSUES_TO_SPRINT_TOOL_NAME,
                ],
            ),
            (
                "jira_service_desk",
                vec![
                    tools::JIRA_GET_SERVICE_DESK_FOR_PROJECT_TOOL_NAME,
                    tools::JIRA_GET_SERVICE_DESK_QUEUES_TOOL_NAME,
                    tools::JIRA_GET_QUEUE_ISSUES_TOOL_NAME,
                ],
            ),
            (
                "jira_forms",
                vec![
                    tools::JIRA_GET_ISSUE_PROFORMA_FORMS_TOOL_NAME,
                    tools::JIRA_GET_PROFORMA_FORM_DETAILS_TOOL_NAME,
                    tools::JIRA_UPDATE_PROFORMA_FORM_ANSWERS_TOOL_NAME,
                ],
            ),
            (
                "jira_metrics",
                vec![
                    tools::JIRA_GET_ISSUE_DATES_TOOL_NAME,
                    tools::JIRA_GET_ISSUE_SLA_TOOL_NAME,
                ],
            ),
            (
                "jira_development",
                vec![
                    tools::JIRA_GET_ISSUE_DEVELOPMENT_INFO_TOOL_NAME,
                    tools::JIRA_GET_ISSUES_DEVELOPMENT_INFO_TOOL_NAME,
                ],
            ),
        ];
        let c4_tools = stage_three_c4_tool_names();

        for (toolset, expected) in cases {
            let server = server_with_config(RuntimeConfig {
                jira: Some(jira_config()),
                enabled_toolsets: BTreeSet::from([toolset.to_string()]),
                atlassian_oauth_cloud_id: Some("cloud-123".to_string()),
                ..runtime_config()
            });
            let names = current_tool_names(&server);
            for expected_name in expected {
                assert!(
                    names.contains(&expected_name.to_string()),
                    "{toolset} should expose {expected_name}"
                );
            }
            for name in c4_tools.iter().copied() {
                if tool_registry::metadata_for(name)
                    .and_then(|metadata| metadata.toolset)
                    .is_some_and(|metadata_toolset| metadata_toolset != toolset)
                {
                    assert!(
                        !names.contains(&name.to_string()),
                        "{toolset} should not expose {name}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn c4_product_dependency_responses_are_structured() {
        let (base_url, _requests) = mock_jira_server().await;
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let agile = server
            .jira_get_agile_boards(Parameters(tools::JiraGetAgileBoardsArgs {
                project_key: Some("NOAGILE".to_string()),
                board_type: None,
                start_at: None,
                limit: None,
            }))
            .await
            .unwrap();
        let forms = server
            .jira_get_issue_proforma_forms(Parameters(tools::JiraGetIssueProformaFormsArgs {
                issue_key: "ABC-1".to_string(),
            }))
            .await
            .unwrap();
        let sla = server
            .jira_get_issue_sla(Parameters(tools::JiraGetIssueSlaArgs {
                issue_key: "ABC-1".to_string(),
                metrics: None,
                working_hours_only: None,
                include_raw_dates: None,
            }))
            .await
            .unwrap();

        let (jsm_url, _requests) = mock_jira_server().await;
        let jsm_down = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(format!("{jsm_url}/jsm-down"))),
            ..runtime_config()
        })
        .jira_get_service_desk_for_project(Parameters(tools::JiraGetServiceDeskForProjectArgs {
            project_key: "ABC".to_string(),
        }))
        .await
        .unwrap();

        let (dev_url, _requests) = mock_jira_server().await;
        let dev_down = server_with_config(RuntimeConfig {
            jira: Some(jira_config_with_base_url(format!("{dev_url}/dev-down"))),
            ..runtime_config()
        })
        .jira_get_issue_development_info(Parameters(tools::JiraGetIssueDevelopmentInfoArgs {
            issue_key: "10001".to_string(),
            application_type: None,
            data_type: None,
        }))
        .await
        .unwrap();

        let sla_structured = sla.structured_content.as_ref().unwrap();
        assert_eq!(
            sla_structured["product_dependency"]["available"],
            json!(true),
            "sla"
        );
        assert_eq!(sla_structured["success"], json!(true), "sla");

        for (name, result) in [
            ("agile", agile),
            ("forms", forms),
            ("service_desk", jsm_down),
            ("development", dev_down),
        ] {
            let structured = result.structured_content.as_ref().unwrap();
            if structured.get("success").is_some() {
                assert_eq!(structured["success"], json!(false), "{name}");
            }
            assert_eq!(
                structured["product_dependency"]["available"],
                json!(false),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn read_only_guard_blocks_c4_write_tools_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            atlassian_oauth_cloud_id: Some("cloud-123".to_string()),
            ..runtime_config()
        });
        let write_tools = stage_three_c4_write_tool_names();

        assert_eq!(
            write_tools,
            vec![
                tools::JIRA_CREATE_SPRINT_TOOL_NAME,
                tools::JIRA_UPDATE_SPRINT_TOOL_NAME,
                tools::JIRA_ADD_ISSUES_TO_SPRINT_TOOL_NAME,
                tools::JIRA_UPDATE_PROFORMA_FORM_ANSWERS_TOOL_NAME,
            ]
        );
        for name in write_tools {
            let error = read_only_server
                .guard_registered_tool_call(name)
                .unwrap_err();
            assert_eq!(error.message, "tool is disabled in read-only mode");
        }
        let requests = requests.lock().await;

        assert!(requests.is_empty());
    }

    #[test]
    fn project_read_tools_remain_visible_in_read_only_mode() {
        let read_only_projects = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_projects".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_projects);

        assert!(names.contains(&tools::JIRA_GET_ALL_PROJECTS_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_PROJECT_VERSIONS_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_PROJECT_COMPONENTS_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_CREATE_VERSION_TOOL_NAME.to_string()));
    }

    #[test]
    fn user_profile_tool_remains_visible_in_read_only_mode() {
        let read_only_users = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_users".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_users);

        assert!(names.contains(&tools::JIRA_GET_USER_PROFILE_TOOL_NAME.to_string()));
    }

    #[test]
    fn watcher_read_tool_remains_visible_and_writes_hide_in_read_only_mode() {
        let read_only_watchers = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_watchers".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_watchers);

        assert!(names.contains(&tools::JIRA_GET_ISSUE_WATCHERS_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_ADD_WATCHER_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_REMOVE_WATCHER_TOOL_NAME.to_string()));
    }

    #[test]
    fn worklog_read_tool_remains_visible_in_read_only_mode() {
        let read_only_worklog = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_worklog".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_worklog);

        assert!(names.contains(&tools::JIRA_GET_WORKLOG_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_ADD_WORKLOG_TOOL_NAME.to_string()));
    }

    #[test]
    fn link_read_tool_remains_visible_and_epic_write_hides_in_read_only_mode() {
        let read_only_links = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_links".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_links);

        assert!(names.contains(&tools::JIRA_GET_LINK_TYPES_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_LINK_TO_EPIC_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_CREATE_ISSUE_LINK_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_CREATE_REMOTE_ISSUE_LINK_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_REMOVE_ISSUE_LINK_TOOL_NAME.to_string()));
    }

    #[test]
    fn attachment_read_tools_remain_visible_in_read_only_mode() {
        let read_only_attachments = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_attachments".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_attachments);

        assert!(names.contains(&tools::JIRA_DOWNLOAD_ATTACHMENTS_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_ISSUE_IMAGES_TOOL_NAME.to_string()));
    }

    #[test]
    fn agile_read_tools_remain_visible_in_read_only_mode() {
        let read_only_agile = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_agile".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_agile);

        assert!(names.contains(&tools::JIRA_GET_AGILE_BOARDS_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_BOARD_ISSUES_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_SPRINTS_FROM_BOARD_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_SPRINT_ISSUES_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_CREATE_SPRINT_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_UPDATE_SPRINT_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_ADD_ISSUES_TO_SPRINT_TOOL_NAME.to_string()));
    }

    #[test]
    fn service_desk_read_tools_remain_visible_in_read_only_mode() {
        let read_only_service_desk = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_service_desk".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_service_desk);

        assert!(names.contains(&tools::JIRA_GET_SERVICE_DESK_FOR_PROJECT_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_SERVICE_DESK_QUEUES_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_QUEUE_ISSUES_TOOL_NAME.to_string()));
    }

    #[test]
    fn forms_read_tools_remain_visible_in_read_only_mode() {
        let read_only_forms = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_forms".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_forms);

        assert!(names.contains(&tools::JIRA_GET_ISSUE_PROFORMA_FORMS_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_PROFORMA_FORM_DETAILS_TOOL_NAME.to_string()));
        assert!(!names.contains(&tools::JIRA_UPDATE_PROFORMA_FORM_ANSWERS_TOOL_NAME.to_string()));
    }

    #[test]
    fn metrics_date_tool_remains_visible_in_read_only_mode() {
        let read_only_metrics = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_metrics".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_metrics);

        assert!(names.contains(&tools::JIRA_GET_ISSUE_DATES_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_ISSUE_SLA_TOOL_NAME.to_string()));
    }

    #[test]
    fn development_read_tools_remain_visible_in_read_only_mode() {
        let read_only_development = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_development".to_string()]),
            ..runtime_config()
        });
        let names = current_tool_names(&read_only_development);

        assert!(names.contains(&tools::JIRA_GET_ISSUE_DEVELOPMENT_INFO_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_ISSUES_DEVELOPMENT_INFO_TOOL_NAME.to_string()));
    }

    #[test]
    fn default_jira_tool_schemas_are_client_compatible() {
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from([
                "jira_issues".to_string(),
                "jira_fields".to_string(),
                "jira_comments".to_string(),
                "jira_transitions".to_string(),
            ]),
            ..runtime_config()
        });
        let tools = server.current_tools_result().tools;
        let names = tool_names(tools.clone());

        assert!(names.contains(&tools::JIRA_GET_ISSUE_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_SEARCH_TOOL_NAME.to_string()));
        assert_client_compatible_tool_schemas(&tools);
    }

    #[test]
    fn all_jira_tool_schemas_are_client_compatible() {
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            ..runtime_config()
        });
        let tools = server.current_tools_result().tools;
        let names = tool_names(tools.clone());

        assert!(names.contains(&tools::JIRA_GET_ISSUE_TOOL_NAME.to_string()));
        assert!(names.contains(&tools::JIRA_GET_ISSUE_SLA_TOOL_NAME.to_string()));
        assert_client_compatible_tool_schemas(&tools);
    }

    #[test]
    fn confluence_scaffold_routes_are_discoverable_with_registered_metadata() {
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config()),
            ..runtime_config()
        });
        let names = current_tool_names(&server);

        assert!(names.contains(&confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME.to_string()));
        assert!(names.contains(&confluence_tools::CONFLUENCE_CREATE_PAGE_TOOL_NAME.to_string()));
        assert!(
            server
                .get_tool(confluence_tools::CONFLUENCE_GET_SPACE_PAGE_TREE_TOOL_NAME)
                .is_some()
        );
        for name in confluence_tools::STAGE4_CONFLUENCE_TOOL_NAMES {
            assert!(
                tool_registry::metadata_for(name).is_some(),
                "{name} should have registered metadata"
            );
        }
    }

    #[test]
    fn confluence_default_toolsets_obey_read_only_and_have_client_compatible_schemas() {
        let read_write = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config()),
            enabled_toolsets: BTreeSet::from([
                "confluence_pages".to_string(),
                "confluence_comments".to_string(),
            ]),
            ..runtime_config()
        });
        let read_only = server_with_config(RuntimeConfig {
            read_only: true,
            confluence: Some(confluence_config()),
            enabled_toolsets: BTreeSet::from([
                "confluence_pages".to_string(),
                "confluence_comments".to_string(),
            ]),
            ..runtime_config()
        });
        let read_write_tools = read_write.current_tools_result().tools;
        let read_write_names = tool_names(read_write_tools.clone());
        let read_only_names = current_tool_names(&read_only);

        assert!(
            read_write_names.contains(&confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME.to_string())
        );
        assert!(
            read_write_names
                .contains(&confluence_tools::CONFLUENCE_CREATE_PAGE_TOOL_NAME.to_string())
        );
        assert!(
            read_write_names
                .contains(&confluence_tools::CONFLUENCE_ADD_COMMENT_TOOL_NAME.to_string())
        );
        assert!(
            read_only_names.contains(&confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME.to_string())
        );
        assert!(
            !read_only_names
                .contains(&confluence_tools::CONFLUENCE_CREATE_PAGE_TOOL_NAME.to_string())
        );
        assert!(
            !read_only_names
                .contains(&confluence_tools::CONFLUENCE_ADD_COMMENT_TOOL_NAME.to_string())
        );
        assert_client_compatible_tool_schemas(&read_write_tools);
    }

    #[test]
    fn confluence_c2_toolsets_are_exact_at_mcp_boundary() {
        let read_write = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config()),
            enabled_toolsets: BTreeSet::from([
                "confluence_pages".to_string(),
                "confluence_comments".to_string(),
            ]),
            ..runtime_config()
        });
        let read_only = server_with_config(RuntimeConfig {
            read_only: true,
            confluence: Some(confluence_config()),
            enabled_toolsets: BTreeSet::from([
                "confluence_pages".to_string(),
                "confluence_comments".to_string(),
            ]),
            ..runtime_config()
        });
        let unknown_only = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config()),
            enabled_toolsets: BTreeSet::from(["confluence_unknown".to_string()]),
            ..runtime_config()
        });
        let read_write_names = current_tool_names(&read_write);
        let read_only_names = current_tool_names(&read_only);

        for expected in [
            confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_PAGE_CHILDREN_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_SPACE_PAGE_TREE_TOOL_NAME,
            confluence_tools::CONFLUENCE_CREATE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_UPDATE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_DELETE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_MOVE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_COMMENTS_TOOL_NAME,
            confluence_tools::CONFLUENCE_ADD_COMMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_REPLY_TO_COMMENT_TOOL_NAME,
        ] {
            assert!(
                read_write_names.contains(&expected.to_string()),
                "{expected} should be visible in C2 read/write"
            );
        }
        assert!(
            !read_write_names
                .contains(&confluence_tools::CONFLUENCE_GET_LABELS_TOOL_NAME.to_string())
        );
        for expected in [
            confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_PAGE_CHILDREN_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_SPACE_PAGE_TREE_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_COMMENTS_TOOL_NAME,
        ] {
            assert!(
                read_only_names.contains(&expected.to_string()),
                "{expected} should remain visible in C2 read-only"
            );
        }
        for blocked in [
            confluence_tools::CONFLUENCE_CREATE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_UPDATE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_DELETE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_MOVE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_ADD_COMMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_REPLY_TO_COMMENT_TOOL_NAME,
        ] {
            assert!(
                !read_only_names.contains(&blocked.to_string()),
                "{blocked} should be hidden in C2 read-only"
            );
            assert_eq!(
                read_only
                    .guard_registered_tool_call(blocked)
                    .unwrap_err()
                    .message,
                "tool is disabled in read-only mode"
            );
        }
        assert_eq!(
            current_tool_names(&unknown_only),
            vec![MIGRATION_STATUS_TOOL_NAME.to_string()]
        );
        assert!(
            unknown_only
                .guard_registered_tool_call(confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME)
                .is_err()
        );
    }

    #[test]
    fn confluence_attachments_toolset_obeys_read_only_at_mcp_boundary() {
        let read_write = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config()),
            enabled_toolsets: BTreeSet::from(["confluence_attachments".to_string()]),
            ..runtime_config()
        });
        let read_only = server_with_config(RuntimeConfig {
            read_only: true,
            confluence: Some(confluence_config()),
            enabled_toolsets: BTreeSet::from(["confluence_attachments".to_string()]),
            ..runtime_config()
        });
        let read_write_tools = read_write.current_tools_result().tools;
        let read_write_names = tool_names(read_write_tools.clone());
        let read_only_names = current_tool_names(&read_only);

        for expected in [
            confluence_tools::CONFLUENCE_UPLOAD_ATTACHMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_UPLOAD_ATTACHMENTS_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_ATTACHMENTS_TOOL_NAME,
            confluence_tools::CONFLUENCE_DOWNLOAD_ATTACHMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_DOWNLOAD_CONTENT_ATTACHMENTS_TOOL_NAME,
            confluence_tools::CONFLUENCE_DELETE_ATTACHMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_PAGE_IMAGES_TOOL_NAME,
        ] {
            assert!(
                read_write_names.contains(&expected.to_string()),
                "{expected} should be visible for confluence_attachments"
            );
        }
        assert!(
            !read_write_names.contains(&confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME.to_string())
        );

        for expected in [
            confluence_tools::CONFLUENCE_GET_ATTACHMENTS_TOOL_NAME,
            confluence_tools::CONFLUENCE_DOWNLOAD_ATTACHMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_DOWNLOAD_CONTENT_ATTACHMENTS_TOOL_NAME,
            confluence_tools::CONFLUENCE_GET_PAGE_IMAGES_TOOL_NAME,
        ] {
            assert!(
                read_only_names.contains(&expected.to_string()),
                "{expected} should remain visible in read-only"
            );
        }
        for blocked in [
            confluence_tools::CONFLUENCE_UPLOAD_ATTACHMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_UPLOAD_ATTACHMENTS_TOOL_NAME,
            confluence_tools::CONFLUENCE_DELETE_ATTACHMENT_TOOL_NAME,
        ] {
            assert!(
                !read_only_names.contains(&blocked.to_string()),
                "{blocked} should be hidden in read-only"
            );
            assert_eq!(
                read_only
                    .guard_registered_tool_call(blocked)
                    .unwrap_err()
                    .message,
                "tool is disabled in read-only mode"
            );
        }
        assert_client_compatible_tool_schemas(&read_write_tools);
    }

    #[test]
    fn confluence_enabled_tools_filter_and_direct_call_guard_use_registered_metadata() {
        let unavailable = AtlassianMcpServer::default();
        let search_only = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config()),
            enabled_tools: Some(BTreeSet::from([
                confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME.to_string(),
            ])),
            ..runtime_config()
        });
        let read_only = server_with_config(RuntimeConfig {
            read_only: true,
            confluence: Some(confluence_config()),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });

        assert_eq!(
            current_tool_names(&search_only),
            vec![confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME.to_string()]
        );
        assert!(
            unavailable
                .guard_registered_tool_call(confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME)
                .is_err()
        );
        assert!(
            search_only
                .guard_registered_tool_call(confluence_tools::CONFLUENCE_SEARCH_TOOL_NAME)
                .is_ok()
        );
        assert_eq!(
            read_only
                .guard_registered_tool_call(confluence_tools::CONFLUENCE_CREATE_PAGE_TOOL_NAME)
                .unwrap_err()
                .message,
            "tool is disabled in read-only mode"
        );
    }

    #[tokio::test]
    async fn confluence_search_handler_returns_structured_content_from_mock_rest() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_search(Parameters(confluence_tools::ConfluenceSearchArgs {
                query: "project docs".to_string(),
                limit: Some(10),
                spaces_filter: Some("ENG".to_string()),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["results"][0]["title"], json!("Roadmap"));
        assert_eq!(structured["results"][0]["space"]["key"], json!("ENG"));
        assert_eq!(structured["start"], json!(0));
        assert_eq!(structured["limit"], json!(10));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(
            query_value(&requests[0].path, "limit").as_deref(),
            Some("10")
        );
        assert_eq!(
            query_value(&requests[0].path, "cql").as_deref(),
            Some(r#"(siteSearch ~ "project docs") AND (space = ENG)"#)
        );
    }

    #[tokio::test]
    async fn confluence_search_handler_rejects_invalid_limit_before_http_request() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .confluence_search(Parameters(confluence_tools::ConfluenceSearchArgs {
                query: "project docs".to_string(),
                limit: Some(51),
                spaces_filter: None,
            }))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("limit must be less than or equal to 50")
        );
        assert_eq!(requests.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn confluence_get_page_handler_returns_metadata_by_page_id() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page(Parameters(confluence_tools::ConfluenceGetPageArgs {
                page_id: Some("123".to_string()),
                title: Some("Ignored".to_string()),
                space_key: Some("IGN".to_string()),
                include_metadata: Some(true),
                convert_to_markdown: Some(true),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["metadata"]["id"], json!("123"));
        assert_eq!(structured["metadata"]["title"], json!("Roadmap"));
        assert_eq!(
            structured["metadata"]["content"],
            json!("Roadmap Hello & welcome")
        );
        assert_eq!(structured["metadata"]["version"]["number"], json!(7));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(requests[0].path.starts_with("/rest/api/content/123?"));
        assert!(
            query_value(&requests[0].path, "expand")
                .unwrap()
                .contains("body.storage")
        );
    }

    #[tokio::test]
    async fn confluence_get_page_handler_can_lookup_by_title_and_return_raw_content_only() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page(Parameters(confluence_tools::ConfluenceGetPageArgs {
                page_id: None,
                title: Some("Roadmap".to_string()),
                space_key: Some("ENG".to_string()),
                include_metadata: Some(false),
                convert_to_markdown: Some(false),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["content"]["value"], json!("<p>Raw storage</p>"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(requests[0].path.starts_with("/rest/api/content?"));
        assert_eq!(
            query_value(&requests[0].path, "title").as_deref(),
            Some("Roadmap")
        );
        assert_eq!(
            query_value(&requests[0].path, "spaceKey").as_deref(),
            Some("ENG")
        );
    }

    #[tokio::test]
    async fn confluence_get_page_handler_requires_page_id_or_title_and_space_key() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .confluence_get_page(Parameters(confluence_tools::ConfluenceGetPageArgs {
                page_id: None,
                title: Some("Roadmap".to_string()),
                space_key: None,
                include_metadata: None,
                convert_to_markdown: None,
            }))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("Either page_id OR both title and space_key must be provided")
        );
        assert_eq!(requests.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn confluence_get_page_handler_returns_structured_error_for_missing_page() {
        let (base_url, _requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let by_id = server
            .confluence_get_page(Parameters(confluence_tools::ConfluenceGetPageArgs {
                page_id: Some("missing".to_string()),
                title: None,
                space_key: None,
                include_metadata: None,
                convert_to_markdown: None,
            }))
            .await
            .unwrap();
        let by_title = server
            .confluence_get_page(Parameters(confluence_tools::ConfluenceGetPageArgs {
                page_id: None,
                title: Some("Missing".to_string()),
                space_key: Some("ENG".to_string()),
                include_metadata: None,
                convert_to_markdown: None,
            }))
            .await
            .unwrap();

        assert!(
            by_id.structured_content.as_ref().unwrap()["error"]
                .as_str()
                .unwrap()
                .contains("page not found")
        );
        assert!(
            by_title.structured_content.as_ref().unwrap()["error"]
                .as_str()
                .unwrap()
                .contains("Page with title 'Missing' not found")
        );
    }

    #[tokio::test]
    async fn confluence_get_page_children_handler_returns_pages_and_folders() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_children(Parameters(
                confluence_tools::ConfluenceGetPageChildrenArgs {
                    parent_id: "123".to_string(),
                    expand: Some("version".to_string()),
                    limit: Some(2),
                    include_content: Some(true),
                    convert_to_markdown: Some(true),
                    start: Some(0),
                    include_folders: Some(true),
                },
            ))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["parent_id"], json!("123"));
        assert_eq!(structured["count"], json!(2));
        assert_eq!(structured["results"][0]["title"], json!("Child page"));
        assert_eq!(structured["results"][0]["content"], json!("Child body"));
        assert_eq!(structured["results"][1]["type"], json!("folder"));
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
        assert!(
            query_value(&requests[0].path, "expand")
                .unwrap()
                .contains("body.storage")
        );
    }

    #[tokio::test]
    async fn confluence_get_page_children_handler_rejects_invalid_limit_before_http_request() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .confluence_get_page_children(Parameters(
                confluence_tools::ConfluenceGetPageChildrenArgs {
                    parent_id: "123".to_string(),
                    expand: None,
                    limit: Some(51),
                    include_content: None,
                    convert_to_markdown: None,
                    start: None,
                    include_folders: None,
                },
            ))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("limit must be less than or equal to 50")
        );
        assert_eq!(requests.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn confluence_get_space_page_tree_handler_returns_sorted_flat_tree() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_get_space_page_tree(Parameters(
                confluence_tools::ConfluenceGetSpacePageTreeArgs {
                    space_key: "ENG".to_string(),
                    limit: Some(2),
                },
            ))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["space_key"], json!("ENG"));
        assert_eq!(structured["total_pages"], json!(2));
        assert_eq!(structured["has_more"], json!(false));
        assert_eq!(structured["pages"][0]["id"], json!("100"));
        assert_eq!(structured["pages"][0]["parent_id"], Value::Null);
        assert_eq!(structured["pages"][0]["depth"], json!(0));
        assert_eq!(structured["pages"][1]["parent_id"], json!("100"));
        assert_eq!(structured["pages"][1]["depth"], json!(1));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            query_value(&requests[0].path, "expand").as_deref(),
            Some("ancestors")
        );
    }

    #[tokio::test]
    async fn confluence_get_space_page_tree_handler_reports_truncation_hint() {
        let (base_url, _requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_get_space_page_tree(Parameters(
                confluence_tools::ConfluenceGetSpacePageTreeArgs {
                    space_key: "ENG".to_string(),
                    limit: Some(1),
                },
            ))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["total_pages"], json!(1));
        assert_eq!(structured["has_more"], json!(true));
        assert_eq!(structured["next_start"], json!(1));
    }

    #[tokio::test]
    async fn confluence_get_space_page_tree_handler_rejects_invalid_limit_before_http_request() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .confluence_get_space_page_tree(Parameters(
                confluence_tools::ConfluenceGetSpacePageTreeArgs {
                    space_key: "ENG".to_string(),
                    limit: Some(1001),
                },
            ))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("limit must be less than or equal to 1000")
        );
        assert_eq!(requests.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn confluence_create_page_handler_posts_storage_payload() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_create_page(Parameters(confluence_tools::ConfluenceCreatePageArgs {
                space_key: "ENG".to_string(),
                title: "New page".to_string(),
                content: "# Heading".to_string(),
                parent_id: Some("123".to_string()),
                content_format: Some("markdown".to_string()),
                enable_heading_anchors: Some(true),
                include_content: Some(false),
                emoji: Some("note".to_string()),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["message"], json!("Page created successfully"));
        assert_eq!(structured["page"]["id"], json!("900"));
        assert!(structured["page"].get("content").is_none());
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/api/content");
        assert_eq!(requests[0].body["space"]["key"], json!("ENG"));
        assert_eq!(requests[0].body["ancestors"][0]["id"], json!("123"));
        assert_eq!(
            requests[0].body["body"]["storage"]["value"],
            json!("<h1>Heading</h1>")
        );
        assert_eq!(requests[1].method, Method::PUT);
        assert_eq!(
            requests[1].path,
            "/rest/api/content/900/property/emoji-title-published"
        );
        assert_eq!(requests[1].body["value"], json!("note"));
    }

    #[tokio::test]
    async fn confluence_update_page_handler_increments_version_and_preserves_write_options() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_update_page(Parameters(confluence_tools::ConfluenceUpdatePageArgs {
                page_id: "123".to_string(),
                title: "Updated".to_string(),
                content: "<p>Storage</p>".to_string(),
                is_minor_edit: Some(true),
                version_comment: Some("minor update".to_string()),
                parent_id: Some("100".to_string()),
                content_format: Some("storage".to_string()),
                enable_heading_anchors: None,
                include_content: Some(true),
                emoji: None,
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["message"], json!("Page updated successfully"));
        assert_eq!(structured["page"]["title"], json!("Updated"));
        assert_eq!(structured["page"]["content"], json!("<p>Storage</p>"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[1].method, Method::PUT);
        assert_eq!(requests[1].body["version"]["number"], json!(8));
        assert_eq!(requests[1].body["version"]["minorEdit"], json!(true));
        assert_eq!(
            requests[1].body["version"]["message"],
            json!("minor update")
        );
        assert_eq!(requests[1].body["ancestors"][0]["id"], json!("100"));
    }

    #[tokio::test]
    async fn confluence_write_handlers_reject_invalid_content_format_before_http_request() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .confluence_create_page(Parameters(confluence_tools::ConfluenceCreatePageArgs {
                space_key: "ENG".to_string(),
                title: "New page".to_string(),
                content: "body".to_string(),
                parent_id: None,
                content_format: Some("html".to_string()),
                enable_heading_anchors: None,
                include_content: None,
                emoji: None,
            }))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("content_format must be markdown, wiki, or storage")
        );
        assert_eq!(requests.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn confluence_delete_page_handler_returns_success_and_structured_failure() {
        let (base_url, _requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let success = server
            .confluence_delete_page(Parameters(confluence_tools::ConfluenceDeletePageArgs {
                page_id: "123".to_string(),
            }))
            .await
            .unwrap();
        let failure = server
            .confluence_delete_page(Parameters(confluence_tools::ConfluenceDeletePageArgs {
                page_id: "delete-error".to_string(),
            }))
            .await
            .unwrap();

        assert_eq!(
            success.structured_content.as_ref().unwrap()["success"],
            json!(true)
        );
        assert_eq!(
            failure.structured_content.as_ref().unwrap()["success"],
            json!(false)
        );
        assert!(
            failure.structured_content.as_ref().unwrap()["error"]
                .as_str()
                .unwrap()
                .contains("delete failed")
        );
    }

    #[tokio::test]
    async fn confluence_move_page_handler_updates_parent_or_calls_position_endpoint() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let appended = server
            .confluence_move_page(Parameters(confluence_tools::ConfluenceMovePageArgs {
                page_id: "123".to_string(),
                target_parent_id: Some("100".to_string()),
                target_space_key: None,
                position: Some("append".to_string()),
            }))
            .await
            .unwrap();
        let positioned = server
            .confluence_move_page(Parameters(confluence_tools::ConfluenceMovePageArgs {
                page_id: "123".to_string(),
                target_parent_id: Some("999".to_string()),
                target_space_key: None,
                position: Some("above".to_string()),
            }))
            .await
            .unwrap();

        assert_eq!(
            appended.structured_content.as_ref().unwrap()["message"],
            json!("Page moved successfully")
        );
        assert_eq!(
            positioned.structured_content.as_ref().unwrap()["page"]["id"],
            json!("123")
        );
        let requests = requests.lock().await;
        assert_eq!(requests[1].method, Method::PUT);
        assert_eq!(requests[1].body["ancestors"][0]["id"], json!("100"));
        assert!(
            requests
                .iter()
                .any(|request| request.path == "/rest/api/content/123/move/above/999")
        );
    }

    #[tokio::test]
    async fn confluence_move_page_handler_rejects_invalid_position_before_http_request() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = server
            .confluence_move_page(Parameters(confluence_tools::ConfluenceMovePageArgs {
                page_id: "123".to_string(),
                target_parent_id: Some("100".to_string()),
                target_space_key: None,
                position: Some("sideways".to_string()),
            }))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("position must be append, above, or below")
        );
        assert_eq!(requests.lock().await.len(), 0);
    }

    #[test]
    fn confluence_write_tools_are_blocked_by_read_only_guard() {
        let read_only = server_with_config(RuntimeConfig {
            read_only: true,
            confluence: Some(confluence_config()),
            ..runtime_config()
        });

        for name in [
            confluence_tools::CONFLUENCE_CREATE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_UPDATE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_DELETE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_MOVE_PAGE_TOOL_NAME,
            confluence_tools::CONFLUENCE_ADD_COMMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_REPLY_TO_COMMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_ADD_LABEL_TOOL_NAME,
            confluence_tools::CONFLUENCE_UPLOAD_ATTACHMENT_TOOL_NAME,
            confluence_tools::CONFLUENCE_UPLOAD_ATTACHMENTS_TOOL_NAME,
            confluence_tools::CONFLUENCE_DELETE_ATTACHMENT_TOOL_NAME,
        ] {
            assert_eq!(
                read_only
                    .guard_registered_tool_call(name)
                    .unwrap_err()
                    .message,
                "tool is disabled in read-only mode",
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn confluence_get_comments_handler_returns_comment_list_and_empty_list() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let result = server
            .confluence_get_comments(Parameters(confluence_tools::ConfluenceGetCommentsArgs {
                page_id: "123".to_string(),
            }))
            .await
            .unwrap();
        let empty = server
            .confluence_get_comments(Parameters(confluence_tools::ConfluenceGetCommentsArgs {
                page_id: "empty".to_string(),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["page_id"], json!("123"));
        assert_eq!(structured["count"], json!(2));
        assert_eq!(structured["comments"][0]["body"], json!("First comment"));
        assert_eq!(
            structured["comments"][0]["author"]["display_name"],
            json!("Ada")
        );
        assert_eq!(structured["comments"][1]["parent_comment_id"], json!("c-1"));
        assert_eq!(
            empty.structured_content.as_ref().unwrap()["count"],
            json!(0)
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
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
    async fn confluence_add_and_reply_comment_handlers_post_storage_payloads() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let added = server
            .confluence_add_comment(Parameters(confluence_tools::ConfluenceAddCommentArgs {
                page_id: "123".to_string(),
                body: "# Comment".to_string(),
            }))
            .await
            .unwrap();
        let replied = server
            .confluence_reply_to_comment(Parameters(
                confluence_tools::ConfluenceReplyToCommentArgs {
                    comment_id: "c-1".to_string(),
                    body: "Reply body".to_string(),
                },
            ))
            .await
            .unwrap();

        let added_structured = added.structured_content.as_ref().unwrap();
        let replied_structured = replied.structured_content.as_ref().unwrap();
        assert_eq!(added_structured["success"], json!(true));
        assert_eq!(added_structured["comment"]["id"], json!("c-1"));
        assert_eq!(added_structured["comment"]["body"], json!("Comment"));
        assert_eq!(replied_structured["success"], json!(true));
        assert_eq!(
            replied_structured["comment"]["parent_comment_id"],
            json!("c-1")
        );
        assert_eq!(replied_structured["comment"]["body"], json!("Reply body"));

        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/rest/api/content");
        assert_eq!(requests[0].body["type"], json!("comment"));
        assert_eq!(requests[0].body["container"]["id"], json!("123"));
        assert_eq!(requests[0].body["container"]["type"], json!("page"));
        assert_eq!(
            requests[0].body["body"]["storage"]["value"],
            json!("<h1>Comment</h1>")
        );
        assert_eq!(requests[1].body["container"]["id"], json!("c-1"));
        assert_eq!(requests[1].body["container"]["type"], json!("comment"));
        assert_eq!(
            requests[1].body["body"]["storage"]["value"],
            json!("<p>Reply body</p>")
        );
    }

    #[tokio::test]
    async fn confluence_comment_write_handlers_return_structured_failure() {
        let (base_url, _requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let add_failure = server
            .confluence_add_comment(Parameters(confluence_tools::ConfluenceAddCommentArgs {
                page_id: "comment-error".to_string(),
                body: "Comment".to_string(),
            }))
            .await
            .unwrap();
        let reply_failure = server
            .confluence_reply_to_comment(Parameters(
                confluence_tools::ConfluenceReplyToCommentArgs {
                    comment_id: "reply-error".to_string(),
                    body: "Reply".to_string(),
                },
            ))
            .await
            .unwrap();

        for result in [add_failure, reply_failure] {
            let structured = result.structured_content.as_ref().unwrap();
            assert_eq!(structured["success"], json!(false));
            assert!(
                structured["error"]
                    .as_str()
                    .unwrap()
                    .contains("comment failed")
            );
        }
    }

    #[tokio::test]
    async fn confluence_get_labels_handler_returns_label_list_and_empty_list() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_labels(Parameters(confluence_tools::ConfluenceGetLabelsArgs {
                page_id: "123".to_string(),
            }))
            .await
            .unwrap();
        let empty = server
            .confluence_get_labels(Parameters(confluence_tools::ConfluenceGetLabelsArgs {
                page_id: "empty-labels".to_string(),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["content_id"], json!("123"));
        assert_eq!(structured["count"], json!(2));
        assert_eq!(structured["labels"][0]["name"], json!("draft"));
        assert_eq!(structured["labels"][1]["prefix"], json!("my"));
        assert_eq!(
            empty.structured_content.as_ref().unwrap()["count"],
            json!(0)
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[0].path, "/rest/api/content/123/label");
        assert_eq!(requests[1].path, "/rest/api/content/empty-labels/label");
    }

    #[tokio::test]
    async fn confluence_add_label_handler_posts_label_and_refreshes_list() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_add_label(Parameters(confluence_tools::ConfluenceAddLabelArgs {
                page_id: "123".to_string(),
                name: "draft".to_string(),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["message"], json!("Label added successfully"));
        assert_eq!(structured["content_id"], json!("123"));
        assert_eq!(structured["count"], json!(2));
        assert_eq!(structured["labels"][0]["name"], json!("draft"));
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
    async fn confluence_add_label_handler_returns_error_on_api_failure() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let error = server
            .confluence_add_label(Parameters(confluence_tools::ConfluenceAddLabelArgs {
                page_id: "label-error".to_string(),
                name: "draft".to_string(),
            }))
            .await
            .unwrap_err();

        assert!(error.message.contains("label failed"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/rest/api/content/label-error/label");
    }

    #[tokio::test]
    async fn confluence_search_user_handler_wraps_simple_query_for_cloud() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_cloud_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_search_user(Parameters(confluence_tools::ConfluenceSearchUserArgs {
                query: "Ada".to_string(),
                limit: Some(5),
                group_name: None,
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["cql_query"], json!("user.fullname ~ \"Ada\""));
        assert_eq!(structured["count"], json!(1));
        assert_eq!(structured["results"][0]["title"], json!("Ada Lovelace"));
        assert_eq!(structured["results"][0]["user"]["active"], json!(true));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
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
    async fn confluence_search_user_handler_uses_group_member_fallback_on_server() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_search_user(Parameters(confluence_tools::ConfluenceSearchUserArgs {
                query: "Ada".to_string(),
                limit: Some(10),
                group_name: None,
            }))
            .await
            .unwrap();
        let empty = server
            .confluence_search_user(Parameters(confluence_tools::ConfluenceSearchUserArgs {
                query: "Nobody".to_string(),
                limit: Some(10),
                group_name: None,
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["group_name"], json!("confluence-users"));
        assert_eq!(structured["count"], json!(1));
        assert_eq!(structured["results"][0]["title"], json!("Ada Lovelace"));
        assert_eq!(
            empty.structured_content.as_ref().unwrap()["count"],
            json!(0)
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0]
                .path
                .starts_with("/rest/api/group/confluence-users/member?")
        );
        assert_eq!(
            query_value(&requests[0].path, "limit").as_deref(),
            Some("200")
        );
    }

    #[tokio::test]
    async fn confluence_search_user_handler_returns_structured_auth_error() {
        let (base_url, _requests) = mock_confluence_server().await;
        let mut config = confluence_config_with_base_url(base_url);
        config.auth = AtlassianAuth::Pat {
            personal_token: "wrong-token".to_string(),
        };
        let server = server_with_config(RuntimeConfig {
            confluence: Some(config),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_search_user(Parameters(confluence_tools::ConfluenceSearchUserArgs {
                query: "Ada".to_string(),
                limit: Some(10),
                group_name: None,
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(false));
        assert_eq!(structured["status"], json!(401));
        assert!(
            structured["error"]
                .as_str()
                .unwrap()
                .contains("Authentication failed")
        );
    }

    #[tokio::test]
    async fn confluence_search_user_handler_rejects_invalid_limit_before_http_request() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let error = server
            .confluence_search_user(Parameters(confluence_tools::ConfluenceSearchUserArgs {
                query: "Ada".to_string(),
                limit: Some(51),
                group_name: None,
            }))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("limit must be less than or equal to 50")
        );
        assert_eq!(requests.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn confluence_get_page_history_handler_returns_specific_version() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_history(Parameters(
                confluence_tools::ConfluenceGetPageHistoryArgs {
                    page_id: "123".to_string(),
                    version: 1,
                    convert_to_markdown: Some(false),
                },
            ))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["id"], json!("123"));
        assert_eq!(structured["status"], json!("historical"));
        assert_eq!(structured["version"]["number"], json!(1));
        assert_eq!(
            structured["content"],
            json!("<h1>Roadmap</h1><p>Hello team</p>")
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(requests[0].path.starts_with("/rest/api/content/123?"));
        assert_eq!(
            query_value(&requests[0].path, "status").as_deref(),
            Some("historical")
        );
        assert_eq!(
            query_value(&requests[0].path, "version").as_deref(),
            Some("1")
        );
    }

    #[tokio::test]
    async fn confluence_get_page_history_handler_rejects_zero_version_before_http_request() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let error = server
            .confluence_get_page_history(Parameters(
                confluence_tools::ConfluenceGetPageHistoryArgs {
                    page_id: "123".to_string(),
                    version: 0,
                    convert_to_markdown: None,
                },
            ))
            .await
            .unwrap_err();

        assert!(error.message.contains("version must be positive"));
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn confluence_get_page_history_handler_surfaces_missing_version_error() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let error = server
            .confluence_get_page_history(Parameters(
                confluence_tools::ConfluenceGetPageHistoryArgs {
                    page_id: "123".to_string(),
                    version: 99,
                    convert_to_markdown: None,
                },
            ))
            .await
            .unwrap_err();

        assert!(error.message.contains("historical version not found"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            query_value(&requests[0].path, "version").as_deref(),
            Some("99")
        );
    }

    #[tokio::test]
    async fn confluence_get_page_diff_handler_returns_deterministic_diff() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_diff(Parameters(confluence_tools::ConfluenceGetPageDiffArgs {
                page_id: "123".to_string(),
                from_version: 1,
                to_version: 2,
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["page_id"], json!("123"));
        assert_eq!(structured["title"], json!("Roadmap"));
        assert_eq!(structured["has_changes"], json!(true));
        assert_eq!(
            structured["diff"],
            json!(
                "--- v1\n+++ v2\n@@ -1 +1 @@\n-Roadmap Hello team\n+Roadmap Hello team and partners"
            )
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(
            query_value(&requests[0].path, "version").as_deref(),
            Some("1")
        );
        assert_eq!(
            query_value(&requests[1].path, "version").as_deref(),
            Some("2")
        );
    }

    #[tokio::test]
    async fn confluence_get_page_diff_handler_returns_empty_diff_for_same_version() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_diff(Parameters(confluence_tools::ConfluenceGetPageDiffArgs {
                page_id: "123".to_string(),
                from_version: 2,
                to_version: 2,
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["from_version"], json!(2));
        assert_eq!(structured["to_version"], json!(2));
        assert_eq!(structured["diff"], json!(""));
        assert_eq!(structured["has_changes"], json!(false));
        assert_eq!(requests.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn confluence_get_page_diff_handler_rejects_invalid_order_before_http_request() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let error = server
            .confluence_get_page_diff(Parameters(confluence_tools::ConfluenceGetPageDiffArgs {
                page_id: "123".to_string(),
                from_version: 3,
                to_version: 2,
            }))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("from_version must be less than or equal to to_version")
        );
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn confluence_get_page_views_handler_returns_cloud_analytics_with_title() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_cloud_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_views(Parameters(confluence_tools::ConfluenceGetPageViewsArgs {
                page_id: "123".to_string(),
                include_title: Some(true),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["page_id"], json!("123"));
        assert_eq!(structured["page_title"], json!("Roadmap"));
        assert_eq!(structured["total_views"], json!(42));
        assert_eq!(structured["unique_viewers"], json!(7));
        assert_eq!(structured["last_viewed"], json!("2026-06-04T12:00:00Z"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(requests[0].path.starts_with("/rest/api/content/123?"));
        assert_eq!(requests[1].path, "/rest/api/analytics/content/123/views");
    }

    #[tokio::test]
    async fn confluence_get_page_views_handler_skips_title_lookup_when_disabled() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_cloud_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_views(Parameters(confluence_tools::ConfluenceGetPageViewsArgs {
                page_id: "123".to_string(),
                include_title: Some(false),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert!(structured["page_title"].is_null());
        assert_eq!(structured["total_views"], json!(42));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/rest/api/analytics/content/123/views");
    }

    #[tokio::test]
    async fn confluence_get_page_views_handler_returns_unavailable_on_server_without_http() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_views(Parameters(confluence_tools::ConfluenceGetPageViewsArgs {
                page_id: "123".to_string(),
                include_title: Some(true),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(false));
        assert_eq!(structured["available"], json!(false));
        assert!(
            structured["error"]
                .as_str()
                .unwrap()
                .contains("only available for Confluence Cloud")
        );
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn confluence_get_page_views_handler_returns_structured_auth_error() {
        let (base_url, _requests) = mock_confluence_server().await;
        let mut config = confluence_cloud_config_with_base_url(base_url);
        config.auth = AtlassianAuth::Pat {
            personal_token: "wrong-token".to_string(),
        };
        let server = server_with_config(RuntimeConfig {
            confluence: Some(config),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_views(Parameters(confluence_tools::ConfluenceGetPageViewsArgs {
                page_id: "123".to_string(),
                include_title: Some(false),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(false));
        assert_eq!(structured["status"], json!(401));
        assert!(
            structured["error"]
                .as_str()
                .unwrap()
                .contains("Authentication failed")
        );
    }

    #[tokio::test]
    async fn confluence_get_page_views_handler_rejects_empty_page_id_before_http() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_cloud_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let error = server
            .confluence_get_page_views(Parameters(confluence_tools::ConfluenceGetPageViewsArgs {
                page_id: " ".to_string(),
                include_title: None,
            }))
            .await
            .unwrap_err();

        assert!(error.message.contains("page_id must not be empty"));
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn confluence_get_attachments_handler_returns_metadata_page() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_attachments(Parameters(
                confluence_tools::ConfluenceGetAttachmentsArgs {
                    content_id: "123".to_string(),
                    start: Some(0),
                    limit: Some(2),
                    filename: None,
                    media_type: None,
                },
            ))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(true));
        assert_eq!(structured["content_id"], json!("123"));
        assert_eq!(structured["count"], json!(2));
        assert_eq!(structured["attachments"][0]["id"], json!("att-1"));
        assert_eq!(
            structured["attachments"][0]["media_type"],
            json!("image/png")
        );
        assert_eq!(structured["attachments"][0]["file_size"], json!(42));
        assert_eq!(
            structured["attachments"][1]["media_type"],
            json!("text/plain")
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .path
                .starts_with("/rest/api/content/123/child/attachment?")
        );
        assert_eq!(
            query_value(&requests[0].path, "start").as_deref(),
            Some("0")
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
    async fn confluence_get_attachments_handler_handles_empty_and_missing_fields() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let empty = server
            .confluence_get_attachments(Parameters(
                confluence_tools::ConfluenceGetAttachmentsArgs {
                    content_id: "empty-attachments".to_string(),
                    start: None,
                    limit: None,
                    filename: None,
                    media_type: None,
                },
            ))
            .await
            .unwrap();
        let missing_fields = server
            .confluence_get_attachments(Parameters(
                confluence_tools::ConfluenceGetAttachmentsArgs {
                    content_id: "missing-attachment-fields".to_string(),
                    start: None,
                    limit: None,
                    filename: None,
                    media_type: None,
                },
            ))
            .await
            .unwrap();

        assert_eq!(
            empty.structured_content.as_ref().unwrap()["count"],
            json!(0)
        );
        let missing = missing_fields.structured_content.as_ref().unwrap();
        assert_eq!(missing["count"], json!(1));
        assert_eq!(missing["attachments"][0]["id"], json!("att-min"));
        assert!(missing["attachments"][0]["title"].is_null());
        assert!(missing["attachments"][0]["media_type"].is_null());
        assert_eq!(requests.lock().await.len(), 2);
    }

    #[tokio::test]
    async fn confluence_get_attachments_handler_filters_filename_and_media_type() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let by_filename = server
            .confluence_get_attachments(Parameters(
                confluence_tools::ConfluenceGetAttachmentsArgs {
                    content_id: "123".to_string(),
                    start: None,
                    limit: None,
                    filename: Some("file.png".to_string()),
                    media_type: None,
                },
            ))
            .await
            .unwrap();
        let by_media_type = server
            .confluence_get_attachments(Parameters(
                confluence_tools::ConfluenceGetAttachmentsArgs {
                    content_id: "123".to_string(),
                    start: None,
                    limit: None,
                    filename: None,
                    media_type: Some("text/plain".to_string()),
                },
            ))
            .await
            .unwrap();

        let filename = by_filename.structured_content.as_ref().unwrap();
        assert_eq!(filename["count"], json!(1));
        assert_eq!(filename["attachments"][0]["title"], json!("file.png"));
        let media_type = by_media_type.structured_content.as_ref().unwrap();
        assert_eq!(media_type["count"], json!(1));
        assert_eq!(media_type["attachments"][0]["title"], json!("notes.txt"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(query_value(&requests[0].path, "filename").is_none());
        assert!(query_value(&requests[1].path, "media-type").is_none());
    }

    #[tokio::test]
    async fn confluence_get_attachments_handler_rejects_invalid_limit_before_http() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let error = server
            .confluence_get_attachments(Parameters(
                confluence_tools::ConfluenceGetAttachmentsArgs {
                    content_id: "123".to_string(),
                    start: None,
                    limit: Some(101),
                    filename: None,
                    media_type: None,
                },
            ))
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("limit must be less than or equal to 100")
        );
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn confluence_download_attachment_handler_returns_bounded_base64_content() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_download_attachment(Parameters(
                confluence_tools::ConfluenceDownloadAttachmentArgs {
                    attachment_id: "att-1".to_string(),
                },
            ))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(true));
        assert_eq!(structured["attachment"]["id"], json!("att-1"));
        assert_eq!(
            structured["attachment"]["content"],
            json!({
                "encoding": "base64",
                "content_type": "image/png",
                "size": 11,
                "data": "aW1hZ2UtYnl0ZXM="
            })
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(requests[0].path.starts_with("/rest/api/content/att-1?"));
        assert_eq!(
            requests[1].path,
            "/download/attachments/att-1/file.png?token=secret"
        );
    }

    #[tokio::test]
    async fn confluence_download_attachment_handler_reports_metadata_errors_without_fetching() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let no_url = server
            .confluence_download_attachment(Parameters(
                confluence_tools::ConfluenceDownloadAttachmentArgs {
                    attachment_id: "att-no-url".to_string(),
                },
            ))
            .await
            .unwrap();
        let too_large = server
            .confluence_download_attachment(Parameters(
                confluence_tools::ConfluenceDownloadAttachmentArgs {
                    attachment_id: "att-large".to_string(),
                },
            ))
            .await
            .unwrap();

        let no_url = no_url.structured_content.as_ref().unwrap();
        assert_eq!(no_url["success"], json!(false));
        assert!(no_url["error"].as_str().unwrap().contains("download URL"));
        let too_large = too_large.structured_content.as_ref().unwrap();
        assert_eq!(too_large["success"], json!(false));
        assert!(
            too_large["error"]
                .as_str()
                .unwrap()
                .contains("exceeds the inline limit")
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|request| !request.path.contains("/download/"))
        );
    }

    #[tokio::test]
    async fn confluence_download_attachment_handler_rejects_stream_limit_and_cross_origin_url() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let stream_too_large = server
            .confluence_download_attachment(Parameters(
                confluence_tools::ConfluenceDownloadAttachmentArgs {
                    attachment_id: "att-stream-large".to_string(),
                },
            ))
            .await
            .unwrap();
        let cross_origin = server
            .confluence_download_attachment(Parameters(
                confluence_tools::ConfluenceDownloadAttachmentArgs {
                    attachment_id: "att-cross".to_string(),
                },
            ))
            .await
            .unwrap();

        let stream_too_large = stream_too_large.structured_content.as_ref().unwrap();
        assert_eq!(stream_too_large["success"], json!(false));
        assert!(
            stream_too_large["error"]
                .as_str()
                .unwrap()
                .contains("exceeds configured limit")
        );
        let cross_origin = cross_origin.structured_content.as_ref().unwrap();
        assert_eq!(cross_origin["success"], json!(false));
        assert!(
            cross_origin["error"]
                .as_str()
                .unwrap()
                .contains("configured Atlassian base origin")
        );
        assert!(
            !cross_origin["error"]
                .as_str()
                .unwrap()
                .contains("token=secret")
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert!(
            requests
                .iter()
                .any(|request| request.path == "/download/attachments/att-stream-large/large.bin")
        );
        assert!(
            requests
                .iter()
                .all(|request| !request.path.contains("other.example"))
        );
    }

    #[tokio::test]
    async fn confluence_download_content_attachments_handler_returns_partial_failure_summary() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_download_content_attachments(Parameters(
                confluence_tools::ConfluenceDownloadContentAttachmentsArgs {
                    content_id: "download-batch".to_string(),
                },
            ))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(true));
        assert_eq!(structured["summary"]["total"], json!(3));
        assert_eq!(structured["summary"]["downloaded"], json!(1));
        assert_eq!(structured["summary"]["failed"], json!(2));
        assert_eq!(structured["attachments"][0]["id"], json!("att-1"));
        assert_eq!(structured["failed"].as_array().unwrap().len(), 2);
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0]
                .path
                .starts_with("/rest/api/content/download-batch/child/attachment?")
        );
        assert_eq!(
            requests[1].path,
            "/download/attachments/att-1/file.png?token=secret"
        );
    }

    #[tokio::test]
    async fn confluence_get_page_images_handler_filters_non_images_and_uses_extension_fallback() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let result = server
            .confluence_get_page_images(Parameters(confluence_tools::ConfluenceGetPageImagesArgs {
                content_id: "images".to_string(),
            }))
            .await
            .unwrap();

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(true));
        assert_eq!(structured["images_only"], json!(true));
        assert_eq!(structured["count"], json!(2));
        assert_eq!(structured["skipped_non_images"], json!(1));
        assert_eq!(structured["images"][0]["id"], json!("att-1"));
        assert_eq!(
            structured["images"][0]["resolved_mime_type"],
            json!("image/png")
        );
        assert_eq!(structured["images"][1]["id"], json!("att-octet-image"));
        assert_eq!(
            structured["images"][1]["resolved_mime_type"],
            json!("image/jpeg")
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert!(
            requests[0]
                .path
                .starts_with("/rest/api/content/images/child/attachment?")
        );
        assert!(
            requests
                .iter()
                .any(|request| request.path == "/download/attachments/att-1/file.png?token=secret")
        );
        assert!(
            requests
                .iter()
                .any(|request| request.path == "/download/attachments/att-octet-image/photo.jpg")
        );
        assert!(
            requests
                .iter()
                .all(|request| !request.path.contains("notes.txt"))
        );
    }

    fn temp_confluence_upload_file(filename: &str, content: &[u8]) -> String {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("mcp-atlassian-rs-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(filename);
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn remove_temp_confluence_upload_file(file_path: &str) {
        let path = std::path::Path::new(file_path);
        let parent = path.parent().map(ToOwned::to_owned);
        let _ = std::fs::remove_file(path);
        if let Some(parent) = parent {
            let _ = std::fs::remove_dir(parent);
        }
    }

    #[tokio::test]
    async fn confluence_upload_attachment_handler_sends_local_file_as_multipart() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let file_path = temp_confluence_upload_file("upload.txt", b"hello");
        let result = server
            .confluence_upload_attachment(Parameters(
                confluence_tools::ConfluenceUploadAttachmentArgs {
                    content_id: "123".to_string(),
                    file_path: file_path.clone(),
                    comment: Some("Initial upload".to_string()),
                    minor_edit: Some(true),
                },
            ))
            .await
            .unwrap();
        remove_temp_confluence_upload_file(&file_path);

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(true));
        assert_eq!(structured["filename"], json!("upload.txt"));
        assert_eq!(structured["minor_edit"], json!(true));
        assert_eq!(structured["attachment"]["title"], json!("upload.txt"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::PUT);
        assert_eq!(requests[0].path, "/rest/api/content/123/child/attachment");
        let body = requests[0].body.as_str().unwrap();
        assert!(body.contains("name=\"file\"; filename=\"upload.txt\""));
        assert!(body.contains("hello"));
        assert!(body.contains("name=\"comment\""));
        assert!(body.contains("Initial upload"));
        assert!(body.contains("name=\"minorEdit\""));
        assert!(body.contains("true"));
        assert!(!body.contains(&file_path));
    }

    #[tokio::test]
    async fn confluence_upload_attachments_handler_returns_partial_success_summary() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let ok_path = temp_confluence_upload_file("batch-1.txt", b"batch");
        let missing_path = std::env::temp_dir()
            .join("mcp-atlassian-rs-missing-upload.txt")
            .to_string_lossy()
            .into_owned();
        let result = server
            .confluence_upload_attachments(Parameters(
                confluence_tools::ConfluenceUploadAttachmentsArgs {
                    content_id: "123".to_string(),
                    file_paths: format!("{ok_path}, {missing_path}"),
                    comment: Some("Batch upload".to_string()),
                    minor_edit: Some(false),
                },
            ))
            .await
            .unwrap();
        remove_temp_confluence_upload_file(&ok_path);

        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["success"], json!(false));
        assert_eq!(structured["partial_success"], json!(true));
        assert_eq!(structured["summary"]["total"], json!(2));
        assert_eq!(structured["summary"]["uploaded"], json!(1));
        assert_eq!(structured["summary"]["failed"], json!(1));
        assert_eq!(
            structured["attachments"][0]["filename"],
            json!("batch-1.txt")
        );
        assert_eq!(
            structured["failed"][0]["filename"],
            json!("mcp-atlassian-rs-missing-upload.txt")
        );
        assert!(
            structured["failed"][0]["error"]
                .as_str()
                .unwrap()
                .contains("failed to read local file")
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/rest/api/content/123/child/attachment");
    }

    #[tokio::test]
    async fn confluence_delete_attachment_handler_returns_structured_success_and_failure() {
        let (base_url, requests) = mock_confluence_server().await;
        let server = server_with_config(RuntimeConfig {
            confluence: Some(confluence_config_with_base_url(base_url)),
            enabled_toolsets: tool_registry::all_toolsets(),
            ..runtime_config()
        });
        let success = server
            .confluence_delete_attachment(Parameters(
                confluence_tools::ConfluenceDeleteAttachmentArgs {
                    attachment_id: "att-1".to_string(),
                },
            ))
            .await
            .unwrap();
        let failure = server
            .confluence_delete_attachment(Parameters(
                confluence_tools::ConfluenceDeleteAttachmentArgs {
                    attachment_id: "att-delete-error".to_string(),
                },
            ))
            .await
            .unwrap();

        let success = success.structured_content.as_ref().unwrap();
        assert_eq!(success["success"], json!(true));
        assert_eq!(success["attachment_id"], json!("att-1"));
        let failure = failure.structured_content.as_ref().unwrap();
        assert_eq!(failure["success"], json!(false));
        assert_eq!(failure["attachment_id"], json!("att-delete-error"));
        assert!(
            failure["error"]
                .as_str()
                .unwrap()
                .contains("delete attachment failed")
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::DELETE);
        assert_eq!(requests[0].path, "/rest/api/content/att-1");
        assert_eq!(requests[1].method, Method::DELETE);
        assert_eq!(requests[1].path, "/rest/api/content/att-delete-error");
    }

    #[test]
    fn tool_discovery_applies_enabled_tools_filter_to_migration_status() {
        let server = server_with_config(RuntimeConfig {
            enabled_tools: Some(BTreeSet::from(["some_other_tool".to_string()])),
            ..runtime_config()
        });

        assert!(current_tool_names(&server).is_empty());
        assert!(server.get_tool(MIGRATION_STATUS_TOOL_NAME).is_none());
        assert!(
            server
                .guard_registered_tool_call(MIGRATION_STATUS_TOOL_NAME)
                .is_err()
        );
    }

    #[test]
    fn tool_discovery_does_not_apply_toolsets_to_migration_status() {
        let server = server_with_config(RuntimeConfig {
            enabled_toolsets: BTreeSet::new(),
            ..runtime_config()
        });

        assert_eq!(
            current_tool_names(&server),
            vec![MIGRATION_STATUS_TOOL_NAME.to_string()]
        );
    }

    #[test]
    fn tool_discovery_fails_closed_for_unmapped_tools() {
        let server = AtlassianMcpServer::default();
        let tools =
            server.filtered_tools_from([tool(MIGRATION_STATUS_TOOL_NAME), tool("unmapped_tool")]);
        let names: Vec<_> = tools
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();

        assert_eq!(names, vec![MIGRATION_STATUS_TOOL_NAME.to_string()]);
    }

    #[test]
    fn tool_discovery_applies_future_service_and_toolset_policy_at_server_boundary() {
        let unavailable = AtlassianMcpServer::default();
        let available = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            confluence: Some(confluence_config()),
            ..runtime_config()
        });
        let jira_fields_only = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_fields".to_string()]),
            ..runtime_config()
        });

        assert_eq!(
            tool_names(unavailable.filtered_tools_from_with_metadata(
                [
                    tool("stage1_synthetic_jira_read"),
                    tool("stage1_synthetic_confluence_read"),
                ],
                metadata_for_test_tool,
            )),
            Vec::<String>::new()
        );
        assert_eq!(
            tool_names(available.filtered_tools_from_with_metadata(
                [
                    tool("stage1_synthetic_jira_read"),
                    tool("stage1_synthetic_confluence_read"),
                ],
                metadata_for_test_tool,
            )),
            vec![
                "stage1_synthetic_confluence_read".to_string(),
                "stage1_synthetic_jira_read".to_string(),
            ]
        );
        assert!(
            jira_fields_only
                .filtered_tools_from_with_metadata(
                    [tool("stage1_synthetic_jira_read")],
                    metadata_for_test_tool,
                )
                .is_empty()
        );
    }

    #[test]
    fn direct_call_guard_applies_future_read_only_policy_at_server_boundary() {
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            ..runtime_config()
        });
        let read_write_server = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            ..runtime_config()
        });

        let error = read_only_server
            .guard_tool_call_with_metadata(
                "stage1_synthetic_jira_write",
                true,
                metadata_for_test_tool,
            )
            .unwrap_err();

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(
            read_write_server
                .guard_tool_call_with_metadata(
                    "stage1_synthetic_jira_write",
                    true,
                    metadata_for_test_tool,
                )
                .is_ok()
        );
        assert!(
            read_write_server
                .guard_tool_call_with_metadata(
                    "stage1_synthetic_jira_write",
                    false,
                    metadata_for_test_tool,
                )
                .is_err()
        );
    }

    #[test]
    fn stage_three_direct_call_guard_uses_registered_metadata_at_mcp_boundary() {
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config()),
            ..runtime_config()
        });
        let read_write_server = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            ..runtime_config()
        });

        for name in stage_three_write_tool_names() {
            let error = read_only_server
                .guard_tool_call_with_metadata(name, true, tool_registry::metadata_for)
                .unwrap_err();
            assert_eq!(error.message, "tool is disabled in read-only mode");
        }
        assert!(
            read_write_server
                .guard_tool_call_with_metadata(
                    tools::JIRA_BATCH_GET_CHANGELOGS_TOOL_NAME,
                    true,
                    tool_registry::metadata_for,
                )
                .is_ok()
        );
        assert!(
            read_write_server
                .guard_tool_call_with_metadata(
                    tools::JIRA_CREATE_ISSUE_TOOL_NAME,
                    false,
                    tool_registry::metadata_for,
                )
                .is_err()
        );
    }

    #[test]
    fn c3_common_tool_cross_check_lists_all_names_and_routes() {
        let server = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            ..runtime_config()
        });
        let names = stage_three_c3_tool_names();

        assert_eq!(names.len(), 18);
        for name in names {
            let metadata = tool_registry::metadata_for(name)
                .unwrap_or_else(|| panic!("{name} should have metadata"));
            assert_eq!(metadata.service, ToolService::Jira);
            assert!(
                server.get_tool(name).is_some(),
                "{name} should have a route"
            );
        }
    }

    #[test]
    fn c3_toolset_and_enabled_tools_filters_are_exact_at_mcp_boundary() {
        let projects_only = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            enabled_toolsets: BTreeSet::from(["jira_projects".to_string()]),
            ..runtime_config()
        });
        let worklog_only = server_with_config(RuntimeConfig {
            jira: Some(jira_config()),
            enabled_tools: Some(BTreeSet::from([
                tools::JIRA_GET_WORKLOG_TOOL_NAME.to_string()
            ])),
            ..runtime_config()
        });

        assert_eq!(
            current_tool_names(&projects_only),
            vec![
                tools::JIRA_BATCH_CREATE_VERSIONS_TOOL_NAME.to_string(),
                tools::JIRA_CREATE_VERSION_TOOL_NAME.to_string(),
                tools::JIRA_GET_ALL_PROJECTS_TOOL_NAME.to_string(),
                tools::JIRA_GET_PROJECT_COMPONENTS_TOOL_NAME.to_string(),
                tools::JIRA_GET_PROJECT_VERSIONS_TOOL_NAME.to_string(),
                MIGRATION_STATUS_TOOL_NAME.to_string(),
            ]
        );
        assert_eq!(
            current_tool_names(&worklog_only),
            vec![tools::JIRA_GET_WORKLOG_TOOL_NAME.to_string()]
        );
        assert!(
            worklog_only
                .guard_registered_tool_call(tools::JIRA_GET_WORKLOG_TOOL_NAME)
                .is_ok()
        );
        assert!(
            worklog_only
                .guard_registered_tool_call(tools::JIRA_GET_LINK_TYPES_TOOL_NAME)
                .is_err()
        );
    }

    #[tokio::test]
    async fn read_only_guard_blocks_c3_write_tools_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        for name in stage_three_c3_write_tool_names() {
            let error = read_only_server
                .guard_registered_tool_call(name)
                .unwrap_err();
            assert_eq!(error.message, "tool is disabled in read-only mode");
        }
        let requests = requests.lock().await;

        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_real_jira_write_tool_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = read_only_server
            .guard_registered_tool_call(tools::JIRA_ADD_COMMENT_TOOL_NAME)
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_jira_create_issue_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = read_only_server
            .guard_registered_tool_call(tools::JIRA_CREATE_ISSUE_TOOL_NAME)
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_jira_batch_create_issues_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = read_only_server
            .guard_registered_tool_call(tools::JIRA_BATCH_CREATE_ISSUES_TOOL_NAME)
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_jira_update_issue_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = read_only_server
            .guard_registered_tool_call(tools::JIRA_UPDATE_ISSUE_TOOL_NAME)
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_jira_delete_issue_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = read_only_server
            .guard_registered_tool_call(tools::JIRA_DELETE_ISSUE_TOOL_NAME)
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_version_write_tools_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        for name in [
            tools::JIRA_CREATE_VERSION_TOOL_NAME,
            tools::JIRA_BATCH_CREATE_VERSIONS_TOOL_NAME,
        ] {
            let error = read_only_server
                .guard_registered_tool_call(name)
                .unwrap_err();
            assert_eq!(error.message, "tool is disabled in read-only mode");
        }
        let requests = requests.lock().await;

        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_watcher_write_tools_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        for name in [
            tools::JIRA_ADD_WATCHER_TOOL_NAME,
            tools::JIRA_REMOVE_WATCHER_TOOL_NAME,
        ] {
            let error = read_only_server
                .guard_registered_tool_call(name)
                .unwrap_err();
            assert_eq!(error.message, "tool is disabled in read-only mode");
        }
        let requests = requests.lock().await;

        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_jira_add_worklog_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = read_only_server
            .guard_registered_tool_call(tools::JIRA_ADD_WORKLOG_TOOL_NAME)
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_jira_link_to_epic_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });
        let error = read_only_server
            .guard_registered_tool_call(tools::JIRA_LINK_TO_EPIC_TOOL_NAME)
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_issue_link_write_tools_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        for name in [
            tools::JIRA_CREATE_ISSUE_LINK_TOOL_NAME,
            tools::JIRA_CREATE_REMOTE_ISSUE_LINK_TOOL_NAME,
            tools::JIRA_REMOVE_ISSUE_LINK_TOOL_NAME,
        ] {
            let error = read_only_server
                .guard_registered_tool_call(name)
                .unwrap_err();
            assert_eq!(error.message, "tool is disabled in read-only mode");
        }
        let requests = requests.lock().await;

        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_agile_write_tools_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            ..runtime_config()
        });

        for name in [
            tools::JIRA_CREATE_SPRINT_TOOL_NAME,
            tools::JIRA_UPDATE_SPRINT_TOOL_NAME,
            tools::JIRA_ADD_ISSUES_TO_SPRINT_TOOL_NAME,
        ] {
            let error = read_only_server
                .guard_registered_tool_call(name)
                .unwrap_err();
            assert_eq!(error.message, "tool is disabled in read-only mode");
        }
        let requests = requests.lock().await;

        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn read_only_guard_blocks_forms_write_tool_before_http_request() {
        let (base_url, requests) = mock_jira_server().await;
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira: Some(jira_config_with_base_url(base_url)),
            atlassian_oauth_cloud_id: Some("cloud-123".to_string()),
            ..runtime_config()
        });
        let error = read_only_server
            .guard_registered_tool_call(tools::JIRA_UPDATE_PROFORMA_FORM_ANSWERS_TOOL_NAME)
            .unwrap_err();
        let requests = requests.lock().await;

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(requests.is_empty());
    }
}
