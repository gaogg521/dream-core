//! Reading the company directory mirror to map a subtree into a project
//! group's department tree (T6 stage 3).
//!
//! one-org owns `one_departments` (the write target); one-enterprise owns
//! `one_directory_departments` (the company directory mirror, T6 stage 1).
//! Same layer, so this goes through a trait the app wires up — the same
//! arrangement as `CredentialRevoker` / `dream_domain_sso::EnterpriseSync`.

use async_trait::async_trait;

/// One department as the company directory mirror currently has it. Flat;
/// the mapping logic reconstructs the subtree from `parent_external_id`.
/// `Serialize` because this doubles as the admin console's "pick a subtree to
/// map" picker response — same shape, no reason to duplicate it into a
/// separate API DTO.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryDepartmentRef {
    pub external_id: String,
    pub parent_external_id: Option<String>,
    pub name: String,
}

/// Reads the deployment's company directory mirror. Deployment-scoped like
/// `dream_domain_sso::DirectorySink` — the implementation resolves "the company" on
/// its own, so this crate never needs to know an enterprise id.
#[async_trait]
pub trait DirectoryTreeSource: Send + Sync {
    /// Every department currently in the mirror, or empty when there is no
    /// company, no directory provider configured, or no sync has ever
    /// completed. Never partial: same "run the whole thing or say nothing"
    /// contract the mirror itself keeps (see `enterprise_003_directory`).
    async fn directory_departments(&self) -> Vec<DirectoryDepartmentRef>;

    /// The primary department of the person the directory mirror holds under
    /// `external_id`, or `None` when this deployment has no directory, the
    /// person is not in it, or the mirror has no department for them.
    ///
    /// `external_id` is the person's own IdP id (Feishu `open_id`/`union_id`),
    /// the same value `one_sso_identities.external_id` carries — which is what
    /// lets an SSO login be tied back to a directory row (see
    /// `one_directory_people`'s column comment). Deliberately NOT the company's
    /// `tenant_key`: that one is shared by every employee.
    ///
    /// Used by `OrgService::auto_join_after_sso` to place a freshly
    /// SSO-authenticated user into the project group whose department tree was
    /// mapped from the branch of the company directory they actually sit in.
    async fn person_department(&self, external_id: &str) -> Option<String>;
}

/// Default when nothing is wired: personal and standalone installs have no
/// company directory, so mapping always reports "nothing to map".
pub struct NoopDirectoryTreeSource;

#[async_trait]
impl DirectoryTreeSource for NoopDirectoryTreeSource {
    async fn directory_departments(&self) -> Vec<DirectoryDepartmentRef> {
        Vec::new()
    }

    async fn person_department(&self, _external_id: &str) -> Option<String> {
        None
    }
}
