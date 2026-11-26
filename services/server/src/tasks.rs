//! Tasks and their steps in a space's file. Storage only: step content is conversation-class text, so it is never logged.

use crate::auth::now;
use crate::db::DbError;
use crate::ids::new_id;
use crate::space::SpaceStore;
use rusqlite::{params, OptionalExtension};

impl SpaceStore {
    /// Starts a `running` task with the id given (a reply's task is its generation id).
    pub fn create_task(&self, id: &str, kind: &str, user_id: &str, conversation_id: &str, token_cap: u64) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT INTO tasks (id, kind, user_id, conversation_id, status, created_at, token_cap)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6)",
            params![id, kind, user_id, conversation_id, now(), token_cap as i64],
        )?;
        Ok(())
    }

    /// Writes a step's row, `running`, before the step starts. Returns the step's id.
    pub fn begin_step(&self, task_id: &str, ord: u32, kind: &str, role: &str) -> Result<String, DbError> {
        let id = new_id();
        self.conn().execute(
            "INSERT INTO task_steps (id, task_id, ord, kind, role, status, started_at) VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6)",
            params![id, task_id, ord, kind, role, now()],
        )?;
        Ok(id)
    }

    pub fn finish_step(&self, step_id: &str, status: &str, content: Option<&str>, tokens: u64) -> Result<(), DbError> {
        self.conn().execute(
            "UPDATE task_steps SET status = ?2, content = ?3, tokens = ?4, ended_at = ?5 WHERE id = ?1",
            params![step_id, status, content, tokens as i64, now()],
        )?;
        Ok(())
    }

    /// Ends a task; `note` is a code word, never text.
    pub fn finish_task(&self, task_id: &str, status: &str, note: Option<&str>, tool_calls: u32, tokens_used: u64) -> Result<(), DbError> {
        self.conn().execute(
            "UPDATE tasks SET status = ?2, note = ?3, tool_calls = ?4, tokens_used = ?5, ended_at = ?6 WHERE id = ?1",
            params![task_id, status, note, tool_calls, tokens_used as i64, now()],
        )?;
        Ok(())
    }

    /// The id of the user's `running` task of this kind, if any (the `research_busy` check).
    pub fn running_task_of_kind(&self, user_id: &str, kind: &str) -> Result<Option<String>, DbError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM tasks WHERE user_id = ?1 AND kind = ?2 AND status = 'running' LIMIT 1",
                params![user_id, kind],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// On start: tasks (and steps) the last process left `running` did not finish. Returns how many tasks.
pub fn mark_running_interrupted(db: &SpaceStore) -> Result<usize, DbError> {
    let t = now();
    db.conn().execute("UPDATE task_steps SET status = 'interrupted', ended_at = ?1 WHERE status = 'running'", [t])?;
    Ok(db.conn().execute("UPDATE tasks SET status = 'interrupted', ended_at = ?1 WHERE status = 'running'", [t])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SpaceStore {
        SpaceStore::open_in_memory().unwrap()
    }

    fn count(db: &SpaceStore, table: &str) -> i64 {
        db.conn().query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn deleting_a_conversation_removes_its_tasks_and_steps_only() {
        let db = store();
        let (a, b) = (db.create_conversation("u1", "a").unwrap(), db.create_conversation("u1", "b").unwrap());
        for c in [&a, &b] {
            let t = new_id();
            db.create_task(&t, "research", "u1", c, 100).unwrap();
            let s = db.begin_step(&t, 0, "writer", "research.writer").unwrap();
            db.finish_step(&s, "done", Some("draft"), 5).unwrap();
        }
        assert!(db.delete_conversation(&a).unwrap());
        assert_eq!((count(&db, "tasks"), count(&db, "task_steps")), (1, 1));
        let left: String = db.conn().query_row("SELECT conversation_id FROM tasks", [], |r| r.get(0)).unwrap();
        assert_eq!(left, b);
    }

    #[test]
    fn a_running_task_reads_interrupted_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("askbar-tasks-{}", new_id()));
        let path = dir.join("s.db");
        let (t, s);
        {
            let db = SpaceStore::open(&path).unwrap();
            let c = db.create_conversation("u1", "c").unwrap();
            t = new_id();
            db.create_task(&t, "research", "u1", &c, 100).unwrap();
            s = db.begin_step(&t, 0, "split", "research.researcher").unwrap();
        }
        let db = SpaceStore::open(&path).unwrap();
        assert_eq!(mark_running_interrupted(&db).unwrap(), 1);
        let st = |table: &str, id: &str| -> String {
            db.conn().query_row(&format!("SELECT status FROM {table} WHERE id = ?1"), [id], |r| r.get(0)).unwrap()
        };
        assert_eq!((st("tasks", &t).as_str(), st("task_steps", &s).as_str()), ("interrupted", "interrupted"));
        assert_eq!(mark_running_interrupted(&db).unwrap(), 0);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_task_of_kind_matches_user_kind_and_running_only() {
        let db = store();
        let c = db.create_conversation("u1", "c").unwrap();
        assert_eq!(db.running_task_of_kind("u1", "research").unwrap(), None);
        let t = new_id();
        db.create_task(&t, "research", "u1", &c, 100).unwrap();
        assert_eq!(db.running_task_of_kind("u1", "research").unwrap(), Some(t.clone()));
        assert_eq!(db.running_task_of_kind("u1", "agent").unwrap(), None);
        assert_eq!(db.running_task_of_kind("u2", "research").unwrap(), None);
        db.finish_task(&t, "done", None, 2, 50).unwrap();
        assert_eq!(db.running_task_of_kind("u1", "research").unwrap(), None);
    }
}
