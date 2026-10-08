use super::*;

impl PostgresStore {
    pub(crate) async fn register_account(
        &self,
        username: &str,
        username_key: &str,
        password_hash: &str,
    ) -> Result<Account, StoreError> {
        let user_id = UserId::new();
        let tenant_id = TenantId::new();
        let mut tx = self.pool.begin().await?;

        let insert = sqlx::query(
            r#"
            INSERT INTO users (id, username, username_key, password_hash)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(user_id.as_uuid())
        .bind(username)
        .bind(username_key)
        .bind(password_hash)
        .execute(&mut *tx)
        .await;
        if let Err(error) = insert {
            return Err(support::map_conflict(
                error,
                "username is already registered",
            ));
        }

        sqlx::query(
            r#"
            INSERT INTO tenants (id, kind, created_by_user_id)
            VALUES ($1, 'personal', $2)
            "#,
        )
        .bind(tenant_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO personal_tenants (tenant_id, user_id) VALUES ($1, $2)")
            .bind(tenant_id.as_uuid())
            .bind(user_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        Ok(Account {
            id: user_id,
            username: username.to_owned(),
            personal_tenant_id: tenant_id,
        })
    }

    pub(crate) async fn account_credential(
        &self,
        username_key: &str,
    ) -> Result<Option<AccountCredential>, StoreError> {
        let row = sqlx::query(
            r#"
            SELECT u.id, u.username, u.password_hash, pt.tenant_id
            FROM users u
            JOIN personal_tenants pt ON pt.user_id = u.id
            WHERE u.username_key = $1 AND u.status = 'active'
            "#,
        )
        .bind(username_key)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok(AccountCredential {
                account: Account {
                    id: UserId::from_uuid(row.try_get("id")?),
                    username: row.try_get("username")?,
                    personal_tenant_id: TenantId::from_uuid(row.try_get("tenant_id")?),
                },
                password_hash: row.try_get("password_hash")?,
            })
        })
        .transpose()
    }
}
