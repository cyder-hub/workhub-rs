use serde_json::{Value, json};

use crate::{
    context::AppContext,
    gitlab::{
        client::{
            AcceptMergeRequestRequest, CreateBranchRequest, CreateMergeRequestRequest,
            GetMergeRequestRequest, ListMergeRequestCommitsRequest, ListMergeRequestDiffsRequest,
            ListMergeRequestPipelinesRequest, ListMergeRequestsRequest, UpdateMergeRequestRequest,
        },
        tools::*,
    },
};

use super::{
    OperationError, OperationResult, gitlab_client, guard_operation,
    mutation_failure_from_upstream, mutation_success,
};

pub async fn get_current_user(
    context: &AppContext,
    _args: GitlabGetCurrentUserArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_GET_CURRENT_USER_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .get_current_user()
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn get_project(
    context: &AppContext,
    args: GitlabGetProjectArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_GET_PROJECT_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .get_project(&args.project)
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn list_merge_requests(
    context: &AppContext,
    args: GitlabListMergeRequestsArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_LIST_MERGE_REQUESTS_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .list_merge_requests(ListMergeRequestsRequest {
            project: args.project,
            state: args.state,
            author_username: args.author_username,
            reviewer_username: args.reviewer_username,
            source_branch: args.source_branch,
            target_branch: args.target_branch,
            labels: args.labels,
            page: args.page,
            per_page: args.per_page,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn get_merge_request(
    context: &AppContext,
    args: GitlabGetMergeRequestArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_GET_MERGE_REQUEST_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .get_merge_request(GetMergeRequestRequest {
            project: args.project,
            merge_request_iid: args.merge_request_iid,
            include_diverged_commits_count: args.include_diverged_commits_count,
            include_rebase_in_progress: args.include_rebase_in_progress,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn list_merge_request_commits(
    context: &AppContext,
    args: GitlabListMergeRequestCommitsArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_LIST_MERGE_REQUEST_COMMITS_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .list_merge_request_commits(ListMergeRequestCommitsRequest {
            project: args.project,
            merge_request_iid: args.merge_request_iid,
            page: args.page,
            per_page: args.per_page,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn list_merge_request_diffs(
    context: &AppContext,
    args: GitlabListMergeRequestDiffsArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_LIST_MERGE_REQUEST_DIFFS_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .list_merge_request_diffs(ListMergeRequestDiffsRequest {
            project: args.project,
            merge_request_iid: args.merge_request_iid,
            max_diff_bytes: args.max_diff_bytes,
            page: args.page,
            per_page: args.per_page,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn list_merge_request_pipelines(
    context: &AppContext,
    args: GitlabListMergeRequestPipelinesArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_LIST_MERGE_REQUEST_PIPELINES_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .list_merge_request_pipelines(ListMergeRequestPipelinesRequest {
            project: args.project,
            merge_request_iid: args.merge_request_iid,
            page: args.page,
            per_page: args.per_page,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn create_merge_request(
    context: &AppContext,
    args: GitlabCreateMergeRequestArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_CREATE_MERGE_REQUEST_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .create_merge_request(CreateMergeRequestRequest {
            project: args.project,
            source_branch: args.source_branch,
            target_branch: args.target_branch,
            title: args.title,
            description: args.description,
            remove_source_branch: args.remove_source_branch,
            squash: args.squash,
            assignee_ids: args.assignee_ids,
            reviewer_ids: args.reviewer_ids,
            labels: args.labels,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(mutation_success(
        "Merge request created successfully",
        value,
    )))
}

pub async fn update_merge_request(
    context: &AppContext,
    args: GitlabUpdateMergeRequestArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_UPDATE_MERGE_REQUEST_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .update_merge_request(UpdateMergeRequestRequest {
            project: args.project,
            merge_request_iid: args.merge_request_iid,
            title: args.title,
            description: args.description,
            state_event: args.state_event,
            labels: args.labels,
            add_labels: args.add_labels,
            remove_labels: args.remove_labels,
            reviewer_ids: args.reviewer_ids,
            assignee_ids: args.assignee_ids,
            target_branch: args.target_branch,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(mutation_success(
        "Merge request updated successfully",
        value,
    )))
}

pub async fn close_merge_request(
    context: &AppContext,
    args: GitlabCloseMergeRequestArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_CLOSE_MERGE_REQUEST_TOOL_NAME, context)?;
    confirm_iid(args.merge_request_iid, args.confirm_iid)?;
    let project = args.project;
    let merge_request_iid = args.merge_request_iid;
    match gitlab_client(context)?
        .close_merge_request(&project, merge_request_iid)
        .await
    {
        Ok(value) => Ok(structured(mutation_success_with_cleanup_hint(
            "Merge request closed successfully",
            value,
            json!({
                "verified_by": "gitlab mr get",
                "expected_state": "closed",
            }),
        ))),
        Err(error) => Ok(OperationResult::structured_error(with_extra_fields(
            mutation_failure_from_upstream(
                "Error closing merge request",
                &error,
                Some(json!({
                    "verified_by": "gitlab mr get",
                    "expected_state": "closed",
                })),
            ),
            [
                ("project", json!(project)),
                ("merge_request_iid", json!(merge_request_iid)),
            ],
        ))),
    }
}

pub async fn delete_merge_request(
    context: &AppContext,
    args: GitlabDeleteMergeRequestArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_DELETE_MERGE_REQUEST_TOOL_NAME, context)?;
    confirm_iid(args.merge_request_iid, args.confirm_iid)?;
    let project = args.project;
    let merge_request_iid = args.merge_request_iid;
    match gitlab_client(context)?
        .delete_merge_request(&project, merge_request_iid)
        .await
    {
        Ok(value) => Ok(structured(json!({
            "success": true,
            "message": "Merge request deleted successfully",
            "data": {
                "project": project,
                "merge_request_iid": merge_request_iid,
                "result": value,
            },
            "cleanup_hint": {
                "verified_by": "gitlab mr get",
                "expected_error": "not_found",
                "fallback": "gitlab mr close",
            },
            "warnings": [],
        }))),
        Err(error) => Ok(OperationResult::structured_error(with_extra_fields(
            mutation_failure_from_upstream(
                "Error deleting merge request",
                &error,
                Some(json!({
                    "verified_by": "gitlab mr get",
                    "expected_error": "not_found",
                    "fallback": "gitlab mr close",
                })),
            ),
            [
                ("project", json!(project)),
                ("merge_request_iid", json!(merge_request_iid)),
            ],
        ))),
    }
}

pub async fn add_merge_request_note(
    context: &AppContext,
    args: GitlabAddMergeRequestNoteArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_ADD_MERGE_REQUEST_NOTE_TOOL_NAME, context)?;
    let project = args.project;
    let merge_request_iid = args.merge_request_iid;
    let client = gitlab_client(context)?;
    let mut value = match client
        .add_merge_request_note(&project, merge_request_iid, args.body)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return Ok(OperationResult::structured_error(with_extra_fields(
                mutation_failure_from_upstream("Error adding merge request note", &error, None),
                [
                    ("project", json!(project)),
                    ("merge_request_iid", json!(merge_request_iid)),
                ],
            )));
        }
    };
    let note_id = value_u64(&value, "id");
    let mut warnings = Vec::new();
    let mut discussion_id = discussion_id_from_note(&value);
    let mut discussion_lookup_source = if discussion_id.is_some() {
        Some("note_response")
    } else {
        None
    };

    if discussion_id.is_none() {
        match note_id {
            Some(note_id) => match client
                .list_merge_request_discussions(&project, merge_request_iid, Some(1), Some(100))
                .await
            {
                Ok(discussions) => {
                    discussion_id = find_discussion_id_for_note(&discussions, note_id);
                    if discussion_id.is_some() {
                        discussion_lookup_source = Some("discussion_list");
                    } else {
                        warnings.push(format!(
                            "discussion_id was not found for note_id {note_id} in the first discussion page"
                        ));
                    }
                }
                Err(error) => warnings.push(format!(
                    "failed to discover discussion_id for note_id {note_id}: {error}"
                )),
            },
            None => warnings.push("note response did not include a numeric id".to_string()),
        }
    }

    if let Some(object) = value.as_object_mut() {
        object.insert(
            "discussion_id".to_string(),
            discussion_id.clone().unwrap_or(Value::Null),
        );
        object.insert(
            "discussion_lookup".to_string(),
            json!({
                "source": discussion_lookup_source,
                "note_id": note_id,
            }),
        );
    }

    Ok(structured(mutation_success_with_warnings(
        "Merge request note added successfully",
        value,
        warnings,
    )))
}

pub async fn update_merge_request_note(
    context: &AppContext,
    args: GitlabUpdateMergeRequestNoteArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_UPDATE_MERGE_REQUEST_NOTE_TOOL_NAME, context)?;
    let project = args.project;
    let merge_request_iid = args.merge_request_iid;
    let note_id = args.note_id;
    match gitlab_client(context)?
        .update_merge_request_note(&project, merge_request_iid, note_id, args.body)
        .await
    {
        Ok(value) => Ok(structured(mutation_success(
            "Merge request note updated successfully",
            value,
        ))),
        Err(error) => Ok(OperationResult::structured_error(with_extra_fields(
            mutation_failure_from_upstream("Error updating merge request note", &error, None),
            [
                ("project", json!(project)),
                ("merge_request_iid", json!(merge_request_iid)),
                ("note_id", json!(note_id)),
            ],
        ))),
    }
}

pub async fn delete_merge_request_note(
    context: &AppContext,
    args: GitlabDeleteMergeRequestNoteArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_DELETE_MERGE_REQUEST_NOTE_TOOL_NAME, context)?;
    let project = args.project;
    let merge_request_iid = args.merge_request_iid;
    let note_id = args.note_id;
    match gitlab_client(context)?
        .delete_merge_request_note(&project, merge_request_iid, note_id)
        .await
    {
        Ok(value) => Ok(structured(json!({
            "success": true,
            "message": "Merge request note deleted successfully",
            "data": {
                "project": project,
                "merge_request_iid": merge_request_iid,
                "note_id": note_id,
                "result": value,
            },
            "cleanup_hint": {
                "verified_by": "gitlab mr discussion list",
            },
            "warnings": [],
        }))),
        Err(error) => Ok(OperationResult::structured_error(with_extra_fields(
            mutation_failure_from_upstream(
                "Error deleting merge request note",
                &error,
                Some(json!({
                    "verified_by": "gitlab mr discussion list",
                })),
            ),
            [
                ("project", json!(project)),
                ("merge_request_iid", json!(merge_request_iid)),
                ("note_id", json!(note_id)),
            ],
        ))),
    }
}

pub async fn list_merge_request_discussions(
    context: &AppContext,
    args: GitlabListMergeRequestDiscussionsArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_LIST_MERGE_REQUEST_DISCUSSIONS_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .list_merge_request_discussions(
            &args.project,
            args.merge_request_iid,
            args.page,
            args.per_page,
        )
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn reply_merge_request_discussion(
    context: &AppContext,
    args: GitlabReplyMergeRequestDiscussionArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_REPLY_MERGE_REQUEST_DISCUSSION_TOOL_NAME, context)?;
    let project = args.project;
    let merge_request_iid = args.merge_request_iid;
    let discussion_id = args.discussion_id;
    let value = match gitlab_client(context)?
        .reply_merge_request_discussion(&project, merge_request_iid, &discussion_id, args.body)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return Ok(OperationResult::structured_error(with_extra_fields(
                mutation_failure_from_upstream(
                    "Error replying to merge request discussion",
                    &error,
                    Some(json!({
                        "verified_by": "gitlab mr discussion list",
                    })),
                ),
                [
                    ("project", json!(project)),
                    ("merge_request_iid", json!(merge_request_iid)),
                    ("discussion_id", json!(discussion_id)),
                ],
            )));
        }
    };

    Ok(structured(mutation_success(
        "Merge request discussion replied successfully",
        value,
    )))
}

pub async fn resolve_merge_request_discussion(
    context: &AppContext,
    args: GitlabResolveMergeRequestDiscussionArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_RESOLVE_MERGE_REQUEST_DISCUSSION_TOOL_NAME, context)?;
    let project = args.project;
    let merge_request_iid = args.merge_request_iid;
    let discussion_id = args.discussion_id;
    let value = match gitlab_client(context)?
        .resolve_merge_request_discussion(
            &project,
            merge_request_iid,
            &discussion_id,
            args.resolved,
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return Ok(OperationResult::structured_error(with_extra_fields(
                mutation_failure_from_upstream(
                    "Error resolving merge request discussion",
                    &error,
                    Some(json!({
                        "verified_by": "gitlab mr discussion list",
                    })),
                ),
                [
                    ("project", json!(project)),
                    ("merge_request_iid", json!(merge_request_iid)),
                    ("discussion_id", json!(discussion_id)),
                ],
            )));
        }
    };

    Ok(structured(mutation_success(
        "Merge request discussion resolved successfully",
        value,
    )))
}

pub async fn get_merge_request_approval_state(
    context: &AppContext,
    args: GitlabMergeRequestRefArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_GET_MERGE_REQUEST_APPROVAL_STATE_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .get_merge_request_approval_state(&args.project, args.merge_request_iid)
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(wrap_array(value)))
}

pub async fn set_merge_request_approval(
    context: &AppContext,
    args: GitlabSetMergeRequestApprovalArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_SET_MERGE_REQUEST_APPROVAL_TOOL_NAME, context)?;
    let client = gitlab_client(context)?;
    let value = match args.action {
        GitlabMergeRequestApprovalAction::Approve => {
            client
                .approve_merge_request(&args.project, args.merge_request_iid)
                .await
        }
        GitlabMergeRequestApprovalAction::Unapprove => {
            client
                .unapprove_merge_request(&args.project, args.merge_request_iid)
                .await
        }
    }
    .map_err(OperationError::from_upstream)?;

    Ok(structured(mutation_success(
        "Merge request approval updated successfully",
        value,
    )))
}

pub async fn accept_merge_request(
    context: &AppContext,
    args: GitlabAcceptMergeRequestArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_ACCEPT_MERGE_REQUEST_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .accept_merge_request(AcceptMergeRequestRequest {
            project: args.project,
            merge_request_iid: args.merge_request_iid,
            sha: args.sha,
            auto_merge: args.auto_merge,
            squash: args.squash,
            should_remove_source_branch: args.should_remove_source_branch,
            merge_commit_message: args.merge_commit_message,
            squash_commit_message: args.squash_commit_message,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(mutation_success(
        "Merge request accepted successfully",
        value,
    )))
}

pub async fn create_branch(
    context: &AppContext,
    args: GitlabCreateBranchArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_CREATE_BRANCH_TOOL_NAME, context)?;
    let value = gitlab_client(context)?
        .create_branch(CreateBranchRequest {
            project: args.project,
            branch: args.branch,
            ref_name: args.ref_name,
        })
        .await
        .map_err(OperationError::from_upstream)?;

    Ok(structured(mutation_success(
        "Branch created successfully",
        value,
    )))
}

pub async fn delete_branch(
    context: &AppContext,
    args: GitlabDeleteBranchArgs,
) -> Result<OperationResult, OperationError> {
    guard_operation(GITLAB_DELETE_BRANCH_TOOL_NAME, context)?;
    confirm_branch(&args.branch, &args.confirm_branch)?;
    let project = args.project;
    let branch = args.branch;
    match gitlab_client(context)?
        .delete_branch(&project, &branch)
        .await
    {
        Ok(value) => Ok(structured(json!({
            "success": true,
            "message": "Branch deleted successfully",
            "data": {
                "project": project,
                "branch": branch,
                "result": value,
            },
            "cleanup_hint": {
                "verified_by": "GitLab repository branch list or UI",
                "expected_error": "not_found",
            },
            "warnings": [],
        }))),
        Err(error) => Ok(OperationResult::structured_error(with_extra_fields(
            mutation_failure_from_upstream(
                "Error deleting branch",
                &error,
                Some(json!({
                    "verified_by": "GitLab repository branch list or UI",
                    "expected_error": "not_found",
                })),
            ),
            [("project", json!(project)), ("branch", json!(branch))],
        ))),
    }
}

fn structured(value: Value) -> OperationResult {
    OperationResult::success(value)
}

fn mutation_success_with_cleanup_hint(
    message: impl Into<String>,
    data: Value,
    cleanup_hint: Value,
) -> Value {
    let mut value = mutation_success(message, data);
    if let Some(object) = value.as_object_mut() {
        object.insert("cleanup_hint".to_string(), cleanup_hint);
    }
    value
}

fn mutation_success_with_warnings(
    message: impl Into<String>,
    data: Value,
    warnings: Vec<String>,
) -> Value {
    let mut value = mutation_success(message, data);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "warnings".to_string(),
            Value::Array(warnings.into_iter().map(Value::String).collect()),
        );
    }
    value
}

fn wrap_array(value: Value) -> Value {
    match value {
        Value::Array(items) => json!({ "items": items }),
        other => other,
    }
}

fn with_extra_fields(
    mut value: Value,
    fields: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    if let Some(object) = value.as_object_mut() {
        for (key, field_value) in fields {
            object.insert(key.to_string(), field_value);
        }
    }
    value
}

fn value_u64(value: &Value, key: &str) -> Option<u64> {
    match value.get(key)? {
        Value::Number(number) => number.as_u64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn discussion_id_from_note(value: &Value) -> Option<Value> {
    value
        .get("discussion_id")
        .filter(|value| !value.is_null())
        .cloned()
}

fn find_discussion_id_for_note(discussions: &Value, note_id: u64) -> Option<Value> {
    discussions.as_array()?.iter().find_map(|discussion| {
        let notes = discussion.get("notes")?.as_array()?;
        let contains_note = notes
            .iter()
            .any(|note| value_u64(note, "id") == Some(note_id));
        contains_note
            .then(|| discussion.get("id").cloned())
            .flatten()
    })
}

fn confirm_iid(merge_request_iid: u64, confirm_iid: u64) -> Result<(), OperationError> {
    if merge_request_iid == confirm_iid {
        Ok(())
    } else {
        Err(OperationError::invalid_input(
            "confirm_iid must match merge_request_iid",
        ))
    }
}

fn confirm_branch(branch: &str, confirm_branch: &str) -> Result<(), OperationError> {
    let branch = branch.trim();
    let confirm_branch = confirm_branch.trim();
    if branch.is_empty() {
        return Err(OperationError::invalid_input("branch must not be empty"));
    }
    if confirm_branch.is_empty() {
        return Err(OperationError::invalid_input(
            "confirm_branch must not be empty",
        ));
    }
    if branch == confirm_branch {
        Ok(())
    } else {
        Err(OperationError::invalid_input(
            "confirm_branch must match branch",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::{
        config::{HttpConfig, RuntimeConfig},
        gitlab::{config::GitlabConfig, tools::GITLAB_GET_PROJECT_TOOL_NAME},
        operations::OperationErrorCategory,
        tool_registry,
        upstream::{auth::UpstreamAuth, custom_headers::CustomHeaders, proxy::ProxyConfig},
    };

    use super::*;

    #[tokio::test]
    async fn operations_gitlab_guard_ignores_mcp_disabled_tools() {
        let context = gitlab_context(
            BTreeSet::new(),
            BTreeSet::from([GITLAB_GET_PROJECT_TOOL_NAME.to_string()]),
        );

        assert!(guard_operation(GITLAB_GET_PROJECT_TOOL_NAME, &context).is_ok());
    }

    #[tokio::test]
    async fn operations_gitlab_validates_diff_bounds_before_http() {
        let error = list_merge_request_diffs(
            &gitlab_context(BTreeSet::new(), BTreeSet::new()),
            GitlabListMergeRequestDiffsArgs {
                project: "group/project".to_string(),
                merge_request_iid: 7,
                max_diff_bytes: Some(0),
                page: None,
                per_page: None,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.category, OperationErrorCategory::InvalidInput);
        assert!(error.message.contains("max_diff_bytes must be positive"));
    }

    #[tokio::test]
    async fn operations_gitlab_preserves_project_filter_validation() {
        let error = get_project(
            &gitlab_context(
                BTreeSet::from(["group/project".to_string()]),
                BTreeSet::new(),
            ),
            GitlabGetProjectArgs {
                project: "other/project".to_string(),
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.category, OperationErrorCategory::InvalidInput);
        assert!(
            error
                .message
                .contains("GitLab project `other/project` is not allowed")
        );
    }

    #[tokio::test]
    async fn operations_gitlab_accept_validation_ignores_mcp_disabled_tools() {
        let error = accept_merge_request(
            &gitlab_context(
                BTreeSet::new(),
                BTreeSet::from([GITLAB_ACCEPT_MERGE_REQUEST_TOOL_NAME.to_string()]),
            ),
            GitlabAcceptMergeRequestArgs {
                project: "group/project".to_string(),
                merge_request_iid: 7,
                sha: " ".to_string(),
                auto_merge: None,
                squash: None,
                should_remove_source_branch: None,
                merge_commit_message: None,
                squash_commit_message: None,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.category, OperationErrorCategory::InvalidInput);
        assert!(error.message.contains("sha must not be empty"));
    }

    #[tokio::test]
    async fn operations_gitlab_accept_validates_required_sha_before_http() {
        let error = accept_merge_request(
            &gitlab_context(BTreeSet::new(), BTreeSet::new()),
            GitlabAcceptMergeRequestArgs {
                project: "group/project".to_string(),
                merge_request_iid: 7,
                sha: "  ".to_string(),
                auto_merge: None,
                squash: None,
                should_remove_source_branch: None,
                merge_commit_message: None,
                squash_commit_message: None,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.category, OperationErrorCategory::InvalidInput);
        assert!(error.message.contains("sha must not be empty"));
    }

    fn gitlab_context(
        projects_filter: BTreeSet<String>,
        mcp_disabled_tools: BTreeSet<String>,
    ) -> AppContext {
        AppContext::from_config(&RuntimeConfig {
            gitlab: Some(GitlabConfig {
                base_url: "https://gitlab.example".to_string(),
                auth: UpstreamAuth::HeaderToken {
                    header_name: reqwest::header::HeaderName::from_static("private-token"),
                    token: "gitlab-token".to_string(),
                },
                ssl_verify: true,
                proxy: ProxyConfig::default(),
                custom_headers: CustomHeaders::default(),
                mtls: None,
                projects_filter,
                timeout_seconds: 75,
            }),
            mcp_disabled_tools,
            mcp_enabled_toolsets: tool_registry::all_toolsets(),
            http: HttpConfig::default(),
            ..RuntimeConfig::default()
        })
    }
}
