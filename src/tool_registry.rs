use std::collections::BTreeSet;

use rmcp::{ErrorData, model::Tool};

use crate::context::AppContext;

pub const MIGRATION_STATUS_TOOL_NAME: &str = "migration_status";

const TOOL_UNAVAILABLE_MESSAGE: &str = "tool not available";
const READ_ONLY_BLOCK_MESSAGE: &str = "tool is disabled in read-only mode";

const ALL_TOOLSETS: &[&str] = &[
    "jira_issues",
    "jira_fields",
    "jira_comments",
    "jira_transitions",
    "jira_projects",
    "jira_agile",
    "jira_links",
    "jira_worklog",
    "jira_attachments",
    "jira_users",
    "jira_watchers",
    "jira_service_desk",
    "jira_forms",
    "jira_metrics",
    "jira_development",
    "confluence_pages",
    "confluence_comments",
    "confluence_labels",
    "confluence_users",
    "confluence_analytics",
    "confluence_attachments",
];

const DEFAULT_TOOLSETS: &[&str] = &[
    "jira_issues",
    "jira_fields",
    "jira_comments",
    "jira_transitions",
    "confluence_pages",
    "confluence_comments",
];

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolService {
    Migration,
    Jira,
    Confluence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAccess {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolMetadata {
    pub name: &'static str,
    pub service: ToolService,
    pub access: ToolAccess,
    pub toolset: Option<&'static str>,
    pub title: &'static str,
    pub description: &'static str,
}

pub const MIGRATION_STATUS_METADATA: ToolMetadata = ToolMetadata {
    name: MIGRATION_STATUS_TOOL_NAME,
    service: ToolService::Migration,
    access: ToolAccess::Read,
    toolset: None,
    title: "Migration status",
    description: "Reports the current Rust migration state.",
};

const REGISTERED_TOOLS: &[ToolMetadata] = &[MIGRATION_STATUS_METADATA];

pub fn all_toolsets() -> BTreeSet<String> {
    ALL_TOOLSETS
        .iter()
        .map(|toolset| (*toolset).to_string())
        .collect()
}

pub fn default_toolsets() -> BTreeSet<String> {
    DEFAULT_TOOLSETS
        .iter()
        .map(|toolset| (*toolset).to_string())
        .collect()
}

pub fn metadata_for(name: &str) -> Option<ToolMetadata> {
    REGISTERED_TOOLS
        .iter()
        .find(|metadata| metadata.name == name)
        .copied()
}

pub fn visible_tools<I>(tools: I, context: &AppContext) -> Vec<Tool>
where
    I: IntoIterator<Item = Tool>,
{
    visible_tools_with_metadata(tools, context, metadata_for)
}

pub fn visible_tools_with_metadata<I, F>(
    tools: I,
    context: &AppContext,
    metadata_for: F,
) -> Vec<Tool>
where
    I: IntoIterator<Item = Tool>,
    F: Fn(&str) -> Option<ToolMetadata>,
{
    let mut tools: Vec<_> = tools
        .into_iter()
        .filter(|tool| {
            metadata_for(tool.name.as_ref())
                .is_some_and(|metadata| is_discoverable(metadata, context))
        })
        .collect();
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    tools
}

pub fn guard_tool_call(name: &str, context: &AppContext) -> Result<(), ErrorData> {
    guard_tool_call_with_metadata(name, context, metadata_for)
}

pub fn guard_tool_call_with_metadata<F>(
    name: &str,
    context: &AppContext,
    metadata_for: F,
) -> Result<(), ErrorData>
where
    F: Fn(&str) -> Option<ToolMetadata>,
{
    let Some(metadata) = metadata_for(name) else {
        return Err(tool_unavailable_error());
    };

    if !is_name_enabled(metadata, context)
        || !is_service_available(metadata, context)
        || !is_toolset_enabled(metadata, context)
    {
        return Err(tool_unavailable_error());
    }

    if context.read_only() && metadata.access == ToolAccess::Write {
        return Err(ErrorData::invalid_params(READ_ONLY_BLOCK_MESSAGE, None));
    }

    Ok(())
}

fn is_discoverable(metadata: ToolMetadata, context: &AppContext) -> bool {
    is_name_enabled(metadata, context)
        && is_service_available(metadata, context)
        && is_toolset_enabled(metadata, context)
        && !(context.read_only() && metadata.access == ToolAccess::Write)
}

fn is_name_enabled(metadata: ToolMetadata, context: &AppContext) -> bool {
    match context.enabled_tools() {
        Some(enabled_tools) => enabled_tools.contains(metadata.name),
        None => true,
    }
}

fn is_service_available(metadata: ToolMetadata, context: &AppContext) -> bool {
    let availability = context.service_availability();

    match metadata.service {
        ToolService::Migration => true,
        ToolService::Jira => availability.jira,
        ToolService::Confluence => availability.confluence,
    }
}

fn is_toolset_enabled(metadata: ToolMetadata, context: &AppContext) -> bool {
    match metadata.toolset {
        Some(toolset) => context.enabled_toolsets().contains(toolset),
        None => true,
    }
}

fn tool_unavailable_error() -> ErrorData {
    ErrorData::invalid_params(TOOL_UNAVAILABLE_MESSAGE, None)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use rmcp::model::{JsonObject, Tool};

    use crate::{
        config::{HttpConfig, RuntimeConfig},
        context::AppContext,
    };

    use super::*;

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
            _ => metadata_for(name),
        }
    }

    fn tool(name: &'static str) -> Tool {
        Tool::new(name, "", Arc::<JsonObject>::new(Default::default()))
    }

    fn context(config: RuntimeConfig) -> AppContext {
        AppContext::from_config(&config)
    }

    fn runtime_config() -> RuntimeConfig {
        RuntimeConfig {
            http: HttpConfig::default(),
            ..RuntimeConfig::default()
        }
    }

    fn names(tools: Vec<Tool>) -> Vec<String> {
        tools
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    #[test]
    fn baseline_toolsets_match_stage_one_reference() {
        let all = all_toolsets();
        let defaults = default_toolsets();

        assert_eq!(all.len(), 21);
        assert_eq!(defaults.len(), 6);
        assert!(defaults.is_subset(&all));
        assert!(all.contains("jira_issues"));
        assert!(all.contains("jira_development"));
        assert!(all.contains("confluence_pages"));
        assert!(all.contains("confluence_attachments"));
    }

    #[test]
    fn migration_status_metadata_is_control_plane_read_tool() {
        let metadata = metadata_for(MIGRATION_STATUS_TOOL_NAME).unwrap();

        assert_eq!(metadata, MIGRATION_STATUS_METADATA);
        assert_eq!(metadata.service, ToolService::Migration);
        assert_eq!(metadata.access, ToolAccess::Read);
        assert_eq!(metadata.toolset, None);
        assert!(!metadata.title.is_empty());
        assert!(!metadata.description.is_empty());
    }

    #[test]
    fn visible_tools_keep_migration_status_and_drop_unknown_tools() {
        let context = AppContext::default();
        let tools = visible_tools(
            [tool(MIGRATION_STATUS_TOOL_NAME), tool("unknown_tool")],
            &context,
        );

        assert_eq!(names(tools), vec![MIGRATION_STATUS_TOOL_NAME.to_string()]);
    }

    #[test]
    fn toolsets_do_not_hide_migration_status() {
        let config = RuntimeConfig {
            enabled_toolsets: BTreeSet::new(),
            ..runtime_config()
        };
        let context = context(config);

        let tools = visible_tools([tool(MIGRATION_STATUS_TOOL_NAME)], &context);

        assert_eq!(names(tools), vec![MIGRATION_STATUS_TOOL_NAME.to_string()]);
    }

    #[test]
    fn enabled_tools_filter_by_exact_tool_name() {
        let config = RuntimeConfig {
            enabled_tools: Some(BTreeSet::from(["stage1_synthetic_jira_read".to_string()])),
            jira_url: Some("https://jira.example".to_string()),
            ..runtime_config()
        };
        let context = context(config);

        let tools = visible_tools_with_metadata(
            [
                tool(MIGRATION_STATUS_TOOL_NAME),
                tool("stage1_synthetic_jira_read"),
            ],
            &context,
            metadata_for_test_tool,
        );

        assert_eq!(names(tools), vec!["stage1_synthetic_jira_read"]);
    }

    #[test]
    fn service_availability_filters_jira_and_confluence_tools() {
        let unavailable = AppContext::default();
        let available = context(RuntimeConfig {
            jira_url: Some("https://jira.example".to_string()),
            confluence_url: Some("https://confluence.example".to_string()),
            ..runtime_config()
        });

        assert_eq!(
            names(visible_tools_with_metadata(
                [
                    tool("stage1_synthetic_jira_read"),
                    tool("stage1_synthetic_confluence_read"),
                ],
                &unavailable,
                metadata_for_test_tool,
            )),
            Vec::<String>::new()
        );
        assert_eq!(
            names(visible_tools_with_metadata(
                [
                    tool("stage1_synthetic_jira_read"),
                    tool("stage1_synthetic_confluence_read"),
                ],
                &available,
                metadata_for_test_tool,
            )),
            vec![
                "stage1_synthetic_confluence_read".to_string(),
                "stage1_synthetic_jira_read".to_string(),
            ]
        );
    }

    #[test]
    fn toolset_filter_hides_synthetic_tools_outside_enabled_toolsets() {
        let context = context(RuntimeConfig {
            jira_url: Some("https://jira.example".to_string()),
            enabled_toolsets: BTreeSet::from(["jira_fields".to_string()]),
            ..runtime_config()
        });

        let tools = visible_tools_with_metadata(
            [tool("stage1_synthetic_jira_read")],
            &context,
            metadata_for_test_tool,
        );

        assert!(tools.is_empty());
    }

    #[test]
    fn read_only_hides_write_tools_and_direct_call_guard_rejects_them() {
        let context = context(RuntimeConfig {
            read_only: true,
            jira_url: Some("https://jira.example".to_string()),
            ..runtime_config()
        });

        let tools = visible_tools_with_metadata(
            [
                tool("stage1_synthetic_jira_read"),
                tool("stage1_synthetic_jira_write"),
            ],
            &context,
            metadata_for_test_tool,
        );
        let error = guard_tool_call_with_metadata(
            "stage1_synthetic_jira_write",
            &context,
            metadata_for_test_tool,
        )
        .unwrap_err();

        assert_eq!(names(tools), vec!["stage1_synthetic_jira_read"]);
        assert_eq!(error.message, READ_ONLY_BLOCK_MESSAGE);
        assert_eq!(error.data, None);
    }

    #[test]
    fn direct_call_guard_allows_write_tools_when_read_only_is_disabled() {
        let context = context(RuntimeConfig {
            jira_url: Some("https://jira.example".to_string()),
            ..runtime_config()
        });

        assert!(
            guard_tool_call_with_metadata(
                "stage1_synthetic_jira_write",
                &context,
                metadata_for_test_tool,
            )
            .is_ok()
        );
    }

    #[test]
    fn direct_call_guard_fails_closed_for_unknown_tools() {
        let error = guard_tool_call_with_metadata(
            "unknown_tool",
            &AppContext::default(),
            metadata_for_test_tool,
        )
        .unwrap_err();

        assert_eq!(error.message, TOOL_UNAVAILABLE_MESSAGE);
        assert_eq!(error.data, None);
    }
}
