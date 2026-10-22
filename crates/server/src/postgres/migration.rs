//! Canonical new migrations and narrowly verified historical newline encodings,
//! retaining SQLx's checksum validation, locking and immutable database history.
use super::StoreError;
use sqlx::{
    PgPool,
    migrate::{Migrate, Migration, Migrator},
};

fn with_sql(migration: &Migration, sql: String) -> Migration {
    Migration::new(
        migration.version,
        migration.description.clone(),
        migration.migration_type,
        sql.into(),
        migration.no_tx,
    )
}

fn match_line_endings(migrator: &mut Migrator, applied: &[sqlx::migrate::AppliedMigration]) {
    for migration in migrator.migrations.to_mut() {
        let lf = migration.sql.replace("\r\n", "\n");
        let canonical = with_sql(migration, lf.clone());
        let Some(previous) = applied
            .iter()
            .find(|entry| entry.version == migration.version)
        else {
            // New history must be portable between platform checkouts. Execute
            // and checksum the same canonical SQL, rather than host line endings.
            *migration = canonical;
            continue;
        };
        let matches = previous.checksum == migration.checksum
            || previous.checksum == canonical.checksum
            || previous.checksum == with_sql(migration, lf.replace('\n', "\r\n")).checksum
            || known_mixed_initial_checksum(
                migration.version,
                &canonical.checksum,
                &previous.checksum,
            );
        *migration = canonical;
        if matches {
            // Change the expectation in this process only. Never overwrite the
            // stored history or accept an unknown checksum / edited SQL.
            migration.checksum = previous.checksum.clone();
        }
    }
}

fn known_mixed_initial_checksum(version: i64, canonical: &[u8], historical: &[u8]) -> bool {
    // The checked-in initial migration historically mixed LF blank lines with
    // CRLF statements on Windows. Bind this exact observed checksum to its
    // normalized SQL hash; changing SQL content invalidates the exception too.
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    version == 1
        && hex(canonical)
            == "820c8a7c8b74f0e6d23934c4baadf23214d388c33c8cd40a43f41dc38b169aaa49e3baa452f181109baf8407cfbee6da"
        && hex(historical)
            == "d65ab221431b9cff43dd710f568172c1673321899ca7373604337e406dcb31b7caf965b8ba7140c291b7b4defe606383"
}

pub(super) async fn run(pool: &PgPool) -> Result<(), StoreError> {
    let mut connection = pool.acquire().await?;
    // A cancelled startup must close the connection and release its session
    // advisory lock, rather than returning a locked connection to the pool.
    connection.close_on_drop();
    connection.lock().await?;
    let result: Result<(), StoreError> = async {
        connection.ensure_migrations_table().await?;
        let applied = connection.list_applied_migrations().await?;
        let mut migrator = sqlx::migrate!("./migrations");
        match_line_endings(&mut migrator, &applied);
        // The same connection already owns SQLx's lock throughout preparation
        // and execution. Keep missing-version and all other checks enabled.
        migrator.set_locking(false);
        migrator.run_direct(&mut *connection).await?;
        Ok(())
    }
    .await;
    let unlocked = connection.unlock().await;
    result?;
    unlocked?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::migrate::AppliedMigration;

    #[sqlx::test(migrations = false)]
    async fn preserves_lf_crlf_history_but_rejects_changed_sql(pool: PgPool) {
        let original = sqlx::migrate!("./migrations");
        let variants: Vec<_> = original
            .iter()
            .enumerate()
            .map(|(index, migration)| {
                let lf = migration.sql.replace("\r\n", "\n");
                with_sql(
                    migration,
                    if index % 2 == 0 {
                        lf
                    } else {
                        lf.replace('\n', "\r\n")
                    },
                )
            })
            .collect();
        let historical = Migrator {
            migrations: variants.into(),
            ..Migrator::DEFAULT
        };
        historical.run(&pool).await.unwrap();
        let before: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        run(&pool).await.unwrap();
        let after: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(before, after, "stored migration history is immutable");
        sqlx::query(
            "UPDATE _sqlx_migrations SET checksum=decode(repeat('ab',48),'hex') WHERE version=1",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(matches!(
            run(&pool).await,
            Err(StoreError::Migration(
                sqlx::migrate::MigrateError::VersionMismatch(1)
            ))
        ));
    }

    #[test]
    fn only_exact_known_newline_variants_are_accepted() {
        let mut migrator = sqlx::migrate!("./migrations");
        let first = migrator.iter().next().unwrap();
        let changed = with_sql(first, first.sql.to_string() + "\n-- changed SQL content\n");
        let original_checksum = with_sql(first, first.sql.replace("\r\n", "\n")).checksum;
        let version = first.version;
        match_line_endings(
            &mut migrator,
            &[AppliedMigration {
                version,
                checksum: changed.checksum.clone(),
            }],
        );
        assert_eq!(migrator.iter().next().unwrap().checksum, original_checksum);
        assert_ne!(migrator.iter().next().unwrap().checksum, changed.checksum);
    }

    #[sqlx::test(migrations = false)]
    async fn new_history_is_canonical_and_concurrent_startups_share_the_migration_lock(
        pool: PgPool,
    ) {
        let (first, second) = tokio::join!(run(&pool), run(&pool));
        first.unwrap();
        second.unwrap();
        let history: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        for (version, checksum) in history {
            let migration = sqlx::migrate!("./migrations")
                .iter()
                .find(|item| item.version == version)
                .unwrap()
                .clone();
            assert_eq!(
                checksum,
                with_sql(&migration, migration.sql.replace("\r\n", "\n"))
                    .checksum
                    .as_ref()
            );
        }
    }

    #[test]
    fn known_mixed_history_is_bound_to_the_exact_original_sql() {
        let original = sqlx::migrate!("./migrations");
        let historical: Vec<_> = original
            .iter()
            .map(|migration| AppliedMigration {
                version: migration.version,
                checksum: migration.checksum.clone(),
            })
            .collect();
        // Simulate a future Linux checkout with uniformly LF source files.
        let migrations = original
            .iter()
            .map(|migration| with_sql(migration, migration.sql.replace("\r\n", "\n")))
            .collect::<Vec<_>>();
        let mut portable = Migrator {
            migrations: migrations.into(),
            ..Migrator::DEFAULT
        };
        match_line_endings(&mut portable, &historical);
        for (expected, found) in historical.iter().zip(portable.iter()) {
            assert_eq!(expected.checksum, found.checksum);
        }
        let first = portable.migrations.to_mut().first_mut().unwrap();
        *first = with_sql(first, first.sql.to_string() + "\n-- content changed\n");
        match_line_endings(&mut portable, &historical);
        assert_ne!(
            historical[0].checksum,
            portable.iter().next().unwrap().checksum
        );
    }
}
