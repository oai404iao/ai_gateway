//! Read-only database pool observation.

use sqlx::PgPool;

#[derive(Clone)]
pub struct DatabaseHealth {
    pool: PgPool,
}

impl From<PgPool> for DatabaseHealth {
    fn from(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl DatabaseHealth {
    #[must_use]
    pub fn size(&self) -> u32 {
        self.pool.size()
    }

    #[must_use]
    pub fn idle(&self) -> usize {
        self.pool.num_idle()
    }
}
