//! Composition of the enterprise Office service and live account admission.
use dream_core_auth::AuthState;
use dream_domain_devops::{
    DevopsError, OneDevopsRouterState,
    office_service::{OfficeAdmission, OfficeService},
    office_session::OfficeConfig,
};
use std::{path::Path, sync::Arc};

struct MemberAdmission(AuthState);

#[async_trait::async_trait]
impl OfficeAdmission for MemberAdmission {
    async fn check(&self, actor: &str) -> Result<(), DevopsError> {
        let user = self
            .0
            .user_repo
            .find_active_by_id(actor)
            .await
            .map_err(|_| DevopsError::Internal("member admission unavailable".into()))?;
        if user.is_none() {
            return Err(DevopsError::Forbidden("member is no longer active".into()));
        }
        if let Some(gate) = &self.0.directory_identity {
            if !gate
                .is_allowed(actor)
                .await
                .map_err(|_| DevopsError::Internal("directory admission unavailable".into()))?
            {
                return Err(DevopsError::Forbidden("directory identity is no longer active".into()));
            }
        }
        Ok(())
    }
}

pub(super) fn wire(state: OneDevopsRouterState, auth: &AuthState, data_dir: &Path) -> OneDevopsRouterState {
    let Ok(public) = std::env::var("ONE_OFFICE_PUBLIC_URL") else {
        return state;
    };
    let configured = (|| {
        let required = |name: &str| {
            std::env::var(name).map_err(|_| DevopsError::BadRequest(format!("{name} is required for intranet Office")))
        };
        let secret = required("ONE_OFFICE_JWT_SECRET")?;
        let config = OfficeConfig::new(
            &public,
            &required("ONE_OFFICE_INTERNAL_URL")?,
            &required("ONE_OFFICE_CALLBACK_BASE")?,
            secret.as_bytes(),
        )?;
        OfficeService::new(
            config,
            data_dir.join("knowledge-sources"),
            state.service.clone(),
            Arc::new(MemberAdmission(auth.clone())),
        )
    })();
    match configured {
        Ok(service) => {
            tracing::info!("intranet knowledge Office editing enabled");
            state.with_office(Arc::new(service))
        }
        Err(error) => {
            tracing::error!(%error, "intranet knowledge Office configuration rejected");
            state
        }
    }
}
