use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{ mpsc, RwLock };
use sqlx::{ Pool, Postgres };

pub struct AppState {
    pub rooms: RwLock<HashMap<String, Vec<(String, mpsc::UnboundedSender<String>)>>>,
    pub user_rooms: RwLock<HashMap<String, String>>,
    pub user_senders: RwLock<HashMap<String, mpsc::UnboundedSender<String>>>,
    pub pool: Pool<Postgres>,
    pub redis_client: redis::Client,
}

impl AppState {
    pub fn new(pool: Pool<Postgres>, redis_client: redis::Client) -> Arc<Self> {
        Arc::new(Self {
            rooms: RwLock::new(HashMap::new()),
            user_rooms: RwLock::new(HashMap::new()),
            user_senders: RwLock::new(HashMap::new()),
            pool,
            redis_client,
        })
    }

    pub async fn join_room(&self, username: &str, room: &str, tx: mpsc::UnboundedSender<String>) {
        let mut rooms = self.rooms.write().await;
        for (_, members) in rooms.iter_mut() {
            members.retain(|(name, _)| name != username);
        }
        rooms.entry(room.to_string()).or_default().push((username.to_string(), tx));
        drop(rooms);

        self.user_rooms.write().await.insert(username.to_string(), room.to_string());
    }

    pub async fn leave_all_rooms(&self, username: &str) {
        let mut rooms = self.rooms.write().await;
        for (_, members) in rooms.iter_mut() {
            members.retain(|(name, _)| name != username);
        }
    }

    pub async fn send_to_room(&self, room: &str, msg: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(members) = rooms.get_mut(room) {
            members.retain(|(_, tx)| tx.send(msg.to_string()).is_ok());
        }
    }

    pub async fn get_user_room(&self, username: &str) -> String {
        self.user_rooms
            .read().await
            .get(username)
            .cloned()
            .unwrap_or_else(|| "general".to_string())
    }

    pub async fn get_or_create_db_room(&self, room: &str, created_by: &str) -> i32 {
        sqlx::query(
            "INSERT INTO rooms (name, created_by) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING"
        )
            .bind(room)
            .bind(created_by)
            .execute(&self.pool).await
            .ok();

        sqlx::query_scalar::<_, i32>("SELECT id FROM rooms WHERE name = $1")
            .bind(room)
            .fetch_one(&self.pool).await
            .unwrap_or(1)
    }

    pub async fn list_db_rooms(&self) -> Vec<String> {
        let all: Vec<String> = sqlx
            ::query_scalar::<_, String>("SELECT name FROM rooms ORDER BY name")
            .fetch_all(&self.pool).await
            .unwrap_or_default();
        all.into_iter()
            .filter(|n| !n.starts_with("__dm__"))
            .collect()
    }

    pub async fn register_sender(&self, username: &str, tx: mpsc::UnboundedSender<String>) {
        self.user_senders.write().await.insert(username.to_string(), tx);
    }

    pub async fn unregister_sender(&self, username: &str) {
        self.user_senders.write().await.remove(username);
    }

    pub async fn get_sender(&self, username: &str) -> Option<mpsc::UnboundedSender<String>> {
        self.user_senders.read().await.get(username).cloned()
    }
}
