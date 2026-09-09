use std::process::ExitCode;

use crate::cli::PrepareManagedResourcesArgs;
use crate::commands::error::{CliBoundaryCode, CliBoundaryError};
use dream_core_runtime::managed_resources::export_node_runtime_to_root;
use dream_core_runtime::managed_resources_contract::{
    MANAGED_RESOURCES_CONTRACT_SCHEMA_VERSION, ManagedResourcesContract, validate_contract, write_contract,
};
use dream_core_runtime::node_runtime::managed_node_contract_for_export;
use dream_core_runtime::{
    ManagedAcpToolId, ensure_node_runtime, managed_acp_tool_contract_for_export, prepare_managed_acp_tool_to_root,
};

/// The ACP wrapper packages `factory/acp.rs` actually spawns — the only agent
/// runtime this bundle ships.
///
/// Upstream's native `claude` + `codex` binaries used to be prepared alongside
/// these under `cli/`, on the reasoning that "dropping either list breaks a
/// different thing". That held for `acp/` and not for `cli/`: nothing in this
/// fork spawns the native binaries (`managed_cli` has been dormant since the
/// 2026-07-29 sync, with zero non-test callers of `resolve_bundled_cli`), and
/// the only thing that demanded them — `validate_contract` — runs at packaging
/// time, not at runtime. They cost 696 MB installed on win32-x64, 376 MB of it
/// byte-identical to binaries already inside `acp/codex-acp/`.
const MANAGED_ACP_TOOLS: [ManagedAcpToolId; 2] = [ManagedAcpToolId::ClaudeAgentAcp, ManagedAcpToolId::CodexAcp];

const SUBCOMMAND: &str = "prepare-managed-resources";

pub async fn run_prepare_managed_resources(args: PrepareManagedResourcesArgs) -> Result<ExitCode, CliBoundaryError> {
    let output_root = args.bundle_out;
    std::fs::create_dir_all(&output_root).map_err(|_| prepare_managed_resources_error("output.create"))?;

    let node_runtime = ensure_node_runtime()
        .await
        .map_err(|error| prepare_managed_resources_error_with_detail("node.prepare", error))?;
    let node_dir_name = node_runtime
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| prepare_managed_resources_error("node.layout"))?;
    let exported_node = export_node_runtime_to_root(&output_root, &node_runtime.root, node_dir_name)
        .map_err(|error| prepare_managed_resources_error_with_detail("node.export", error))?;

    println!("Prepared managed resources under {}", output_root.display());
    println!("  node   -> {}", exported_node.display());

    let mut prepared_acp_tools = Vec::new();
    for tool in MANAGED_ACP_TOOLS {
        let prepared = prepare_managed_acp_tool_to_root(tool, &output_root)
            .await
            .map_err(|error| prepare_managed_resources_error_with_detail("acp.prepare", error))?;
        println!("  {:<16} -> {}", tool.slug(), prepared.root.display());
        prepared_acp_tools.push((tool, prepared));
    }

    let node = managed_node_contract_for_export(&output_root, &exported_node)
        .map_err(|error| prepare_managed_resources_error_with_detail("contract.write", error))?;
    // No `cli/` subtree is produced any more; the field stays in the contract so
    // a manifest from an older bundle still deserializes (see REQUIRED_CLI_NAMES).
    let clis = Vec::new();
    let mut acp_tools = Vec::new();
    for (tool, prepared) in &prepared_acp_tools {
        acp_tools.push(
            managed_acp_tool_contract_for_export(*tool, &output_root, prepared)
                .map_err(|error| prepare_managed_resources_error_with_detail("contract.write", error))?,
        );
    }
    // `acpTools[].platformDirectory` carries the same invariant `clis[]` used to
    // (validate_acp_tools_schema rejects any entry that disagrees with runtimeKey),
    // so the key still comes from what was actually prepared rather than from the
    // build host's own platform — cross-arch packaging keeps working.
    let runtime_key = acp_tools
        .first()
        .map(|tool| tool.platform_directory.clone())
        .ok_or_else(|| prepare_managed_resources_error("contract.write"))?;
    let contract = ManagedResourcesContract {
        schema_version: MANAGED_RESOURCES_CONTRACT_SCHEMA_VERSION,
        runtime_key,
        node,
        clis,
        acp_tools,
    };
    let manifest_path = write_contract(&output_root, &contract)
        .map_err(|error| prepare_managed_resources_error_with_detail("contract.write", error))?;
    validate_contract(&output_root, &contract)
        .map_err(|error| prepare_managed_resources_error_with_detail("contract.validate", error))?;
    println!("  manifest -> {}", manifest_path.display());

    Ok(ExitCode::SUCCESS)
}

fn prepare_managed_resources_error(stage: &'static str) -> CliBoundaryError {
    CliBoundaryError::new(
        CliBoundaryCode::CliPrepareManagedResourcesFailed,
        SUBCOMMAND,
        "failed to prepare managed resources",
    )
    .with_field("stage", stage)
}

fn prepare_managed_resources_error_with_detail(stage: &'static str, error: impl std::fmt::Display) -> CliBoundaryError {
    eprintln!("prepare-managed-resources stage={stage} detail: {error}");
    prepare_managed_resources_error(stage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_error_uses_stable_code_and_stage_without_raw_path() {
        let err = prepare_managed_resources_error("node.export");

        assert_eq!(err.code(), CliBoundaryCode::CliPrepareManagedResourcesFailed);
        assert!(err.stderr_line().starts_with(
            "CLI_PREPARE_MANAGED_RESOURCES_FAILED subcommand=prepare-managed-resources stage=node.export"
        ));
        assert!(!err.stderr_line().contains("/Users/secret/bundle"));
    }

    #[test]
    fn prepare_error_accepts_contract_write_and_validate_stages() {
        for stage in ["contract.write", "contract.validate"] {
            let err = prepare_managed_resources_error(stage);
            assert_eq!(err.code(), CliBoundaryCode::CliPrepareManagedResourcesFailed);
            assert!(err.stderr_line().contains(stage));
        }
    }

    #[test]
    fn prepare_error_reports_the_acp_stage_distinctly_from_the_cli_stage() {
        // The two bundling paths fail for unrelated reasons (npm reachability vs
        // native-binary download), so a packaging failure must say which one.
        let acp = prepare_managed_resources_error("acp.prepare");
        let cli = prepare_managed_resources_error("cli.prepare");

        assert!(acp.stderr_line().contains("stage=acp.prepare"));
        assert!(cli.stderr_line().contains("stage=cli.prepare"));
        assert_ne!(acp.stderr_line(), cli.stderr_line());
    }

    #[test]
    fn every_acp_tool_the_runtime_can_spawn_is_prepared() {
        // `ManagedAcpToolId::from_backend` is what `factory/acp.rs` uses to pick a
        // wrapper at session start. Anything reachable there must be in the bundle,
        // otherwise that backend is broken on fresh installs — which is exactly how
        // the 2026-08-07 regression shipped.
        for backend in ["claude", "codex"] {
            let tool = ManagedAcpToolId::from_backend(backend).expect("known backend");
            assert!(
                MANAGED_ACP_TOOLS.contains(&tool),
                "{backend} resolves to {} which is not prepared into the bundle",
                tool.slug()
            );
        }
    }
}
