use sqlx::SqlitePool;

/// Ordered, applied once each, recorded by name. Never edit a shipped entry:
/// add a new one. The baseline is the schema as it first shipped, so a fresh
/// database and an existing one take exactly the same path to the current
/// shape — the only way a migration gets tested before it meets real data.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_baseline",
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            role TEXT NOT NULL CHECK (role IN ('owner','admin','member')),
            locale TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS tokens (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL UNIQUE,
            label TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT
        );
        CREATE TABLE IF NOT EXISTS devices (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            client_id TEXT NOT NULL UNIQUE,
            name TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        -- No rows for an app_id = shared with everyone (default).
        CREATE TABLE IF NOT EXISTS app_shares (
            app_id TEXT NOT NULL,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            PRIMARY KEY (app_id, user_id)
        );
        -- The library. id is ours and authoritative: Wolf honours the id we
        -- send and never persists one of its own.
        CREATE TABLE IF NOT EXISTS games (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            support_hdr BOOLEAN NOT NULL DEFAULT 0,
            icon_png_path TEXT NOT NULL DEFAULT '',
            render_node TEXT NOT NULL DEFAULT '',
            runner_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS engine_defaults (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            h264_gst_pipeline TEXT NOT NULL,
            hevc_gst_pipeline TEXT NOT NULL,
            av1_gst_pipeline TEXT NOT NULL,
            opus_gst_pipeline TEXT NOT NULL,
            start_audio_server BOOLEAN NOT NULL DEFAULT 1,
            start_virtual_compositor BOOLEAN NOT NULL DEFAULT 1
        );",
    ),
    (
        // Engine config is per app, not global: Test ball overrides all four
        // pipelines and turns both servers off. Existing rows keep their data
        // and are backfilled from Wolf on the next connect.
        "0002_per_app_engine_config",
        "ALTER TABLE games ADD COLUMN video_producer_buffer_caps TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN h264_gst_pipeline TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN hevc_gst_pipeline TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN av1_gst_pipeline TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN opus_gst_pipeline TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN start_audio_server BOOLEAN NOT NULL DEFAULT 1;
         ALTER TABLE games ADD COLUMN start_virtual_compositor BOOLEAN NOT NULL DEFAULT 1;
         ALTER TABLE engine_defaults ADD COLUMN video_producer_buffer_caps TEXT NOT NULL DEFAULT '';",
    ),
    (
        // What a game created over the API inherits. Without these a new game
        // gets empty producer caps and no HostConfig, which is precisely the
        // unlaunchable state fixed in 0002.
        "0003_new_game_template",
        "ALTER TABLE engine_defaults ADD COLUMN render_node TEXT NOT NULL DEFAULT '';
         ALTER TABLE engine_defaults ADD COLUMN runner_base_create_json TEXT NOT NULL DEFAULT '';",
    ),
    (
        // What the metadata pipeline discovers. Identity comes from the
        // install folder (goggame-<id>.info / appmanifest_<appid>.acf), so a
        // game knows which store it came from and can be re-looked-up later
        // without asking the admin anything.
        "0004_metadata",
        "ALTER TABLE games ADD COLUMN store TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN store_id TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN slug TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN release_date TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN description TEXT NOT NULL DEFAULT '';",
    ),
    (
        // ProtonDB's compatibility tier. Keyed by Steam appid, which every
        // Proton game already carries as GAMEID=umu-<appid> for protonfixes.
        "0005_protondb",
        "ALTER TABLE games ADD COLUMN protondb_tier TEXT NOT NULL DEFAULT '';",
    ),
    (
        // The Steam appid unlocks art, compatibility and store metadata. It is
        // recorded with HOW it was found, because a title lookup is a guess
        // where a GOG id or a umu GAMEID is exact.
        "0006_steam_identity",
        "ALTER TABLE games ADD COLUMN steam_appid TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN appid_source TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN tagline TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN developer TEXT NOT NULL DEFAULT '';
         ALTER TABLE games ADD COLUMN genres TEXT NOT NULL DEFAULT '';",
    ),
    (
        // Per-title settings. controller_override states a fact about the game
        // (No Man's Sky is XInput-only) where the client-level setting is a
        // per-device preference and cannot express it.
        "0007_per_title_settings",
        "ALTER TABLE games ADD COLUMN controller_override TEXT NOT NULL DEFAULT '';",
    ),
];

pub async fn run(pool: &SqlitePool) {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            name TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await
    .expect("create schema_migrations");

    for (name, sql) in MIGRATIONS {
        let done: Option<String> =
            sqlx::query_scalar("SELECT name FROM schema_migrations WHERE name = ?")
                .bind(name)
                .fetch_optional(pool)
                .await
                .expect("read schema_migrations");
        if done.is_some() {
            continue;
        }
        let mut tx = pool.begin().await.expect("begin migration");
        for statement in sql.split(';').filter(|s| !s.trim().is_empty()) {
            sqlx::query(statement)
                .execute(&mut *tx)
                .await
                .unwrap_or_else(|e| panic!("migration {name} failed on `{statement}`: {e}"));
        }
        sqlx::query("INSERT INTO schema_migrations (name) VALUES (?)")
            .bind(name)
            .execute(&mut *tx)
            .await
            .expect("record migration");
        tx.commit().await.expect("commit migration");
        tracing::info!("db: applied migration {name}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn is_idempotent() {
        let pool = mem().await;
        run(&pool).await;
        run(&pool).await; // second boot must not re-apply
        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied as usize, MIGRATIONS.len());
    }

    #[tokio::test]
    async fn upgrades_an_existing_database_without_losing_rows() {
        let pool = mem().await;
        // Stop at the baseline, insert a row, then bring it forward.
        let (name, sql) = MIGRATIONS[0];
        for statement in sql.split(';').filter(|s| !s.trim().is_empty()) {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "CREATE TABLE schema_migrations (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT (datetime('now')))",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO schema_migrations (name) VALUES (?)")
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO games (id,title,runner_json) VALUES ('7','Kept','{\"type\":\"docker\"}')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run(&pool).await;

        // The row survives, and the new columns exist with their defaults.
        let (title, caps): (String, String) =
            sqlx::query_as("SELECT title, video_producer_buffer_caps FROM games WHERE id='7'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(title, "Kept");
        assert_eq!(caps, "");
    }
}
