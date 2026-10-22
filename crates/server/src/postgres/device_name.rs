use super::*;

impl PostgresStore {
    /// The caller supplies the authenticated endpoint, never a client-supplied
    /// device ID. Row locks serialize renames and endpoint revocation together.
    pub(crate) async fn rename_own_device(
        &self,
        endpoint: &RegisteredEndpoint,
        name: &str,
    ) -> Result<Option<String>, StoreError> {
        let EndpointProofPrincipal::Device { device_id } = endpoint.principal else {
            return Err(StoreError::PermissionDenied);
        };
        let name = support::validate_name(name, "device")?;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT d.name,d.revision,(d.name_changed_at IS NULL OR d.name_changed_at <= clock_timestamp()-interval '10 seconds') AS allowed FROM devices d JOIN endpoints e ON e.device_id=d.id AND e.tenant_id=d.tenant_id JOIN tenants t ON t.id=d.tenant_id WHERE e.endpoint_key=$1 AND d.id=$2 AND d.tenant_id=$3 AND e.status='active' AND d.status='active' AND t.status='active' FOR UPDATE OF d,e")
            .bind(endpoint.endpoint_key.as_bytes().as_slice()).bind(device_id.as_uuid()).bind(endpoint.tenant_id.as_uuid())
            .fetch_optional(&mut *tx).await?.ok_or(StoreError::PermissionDenied)?;
        let previous: String = row.try_get("name")?;
        if previous == name {
            return Ok(Some(name));
        }
        if !row.try_get::<bool, _>("allowed")? {
            return Ok(None);
        }
        let revision: i64 = row.try_get("revision")?;
        sqlx::query("UPDATE devices SET name=$1,revision=revision+1,updated_at=clock_timestamp(),name_changed_at=clock_timestamp() WHERE id=$2")
            .bind(&name).bind(device_id.as_uuid()).execute(&mut *tx).await?;
        let mut record = crate::audit::AuditRecord::new(
            None,
            crate::audit::AuditAction::DeviceRenamed,
            crate::audit::AuditResource::Device(device_id.as_uuid()),
        );
        record.before = Some(crate::audit::AuditValues {
            name: Some(previous),
            revision: Some(revision),
            ..Default::default()
        });
        record.after = Some(crate::audit::AuditValues {
            name: Some(name.clone()),
            revision: Some(revision + 1),
            ..Default::default()
        });
        self.insert_audit(&mut tx, record).await?;
        tx.commit().await?;
        Ok(Some(name))
    }
}
