//! Compose employee reference definitions with the skill registry before serving.
use super::state::RouterBuildError;
use crate::AppServices;

pub(super) async fn seed(services: &AppServices) -> Result<(), RouterBuildError> {
    let guides = dream_domain_employee::catalog_reference::reference_guides(&services.db)
        .await
        .map_err(|error| {
            RouterBuildError::new(
                "router.catalog_reference.read",
                "failed to read builtin employee guides",
            )
            .with_source(error)
        })?;
    let devops = dream_domain_devops::DevopsService::new(services.db.clone());
    let mut inserted = 0;
    for guide in guides {
        inserted += devops
            .ensure_catalog_guide(&guide.id, &guide.name, &guide.description, &guide.content)
            .await
            .map_err(|error| {
                RouterBuildError::new(
                    "router.catalog_reference.skills",
                    "failed to seed builtin employee skills",
                )
                .with_source(error)
            })?;
    }
    let repaired = dream_domain_employee::catalog_reference::backfill_reference_bindings(&services.db)
        .await
        .map_err(|error| {
            RouterBuildError::new(
                "router.catalog_reference.bindings",
                "failed to repair untouched catalog employee bindings",
            )
            .with_source(error)
        })?;
    if inserted > 0 || repaired > 0 {
        tracing::info!(
            inserted,
            repaired,
            "bootstrap: ensured builtin employee reference skills and examples"
        );
    }
    Ok(())
}
