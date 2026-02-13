use lockfree::queue::Queue;
use std::sync::{Arc, Weak};

pub trait ConnectionManager {
    type Conn;
    type Error;

    async fn create_connection(&self) -> Result<Self::Conn, Self::Error>;
}

pub struct ConnPool<T: ConnectionManager> {
    connections: Arc<Queue<T::Conn>>,
    manager: T,
    timeout_duration: std::time::Duration,
}

pub struct Connection<T> {
    inner: core::mem::ManuallyDrop<T>,
    conns_ref: Weak<Queue<T>>,
    valid: bool,
}

impl<T> Connection<T> {
    fn new(conn: T, cref: &Arc<Queue<T>>) -> Self {
        Connection {
            inner: core::mem::ManuallyDrop::new(conn),
            conns_ref: Arc::downgrade(cref),
            valid: true,
        }
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
    }
}

impl<T> Drop for Connection<T> {
    fn drop(&mut self) {
        use core::mem::ManuallyDrop;

        match self.conns_ref.upgrade() {
            Some(upg) if self.valid => {
                let conn = unsafe { ManuallyDrop::take(&mut self.inner) };
                upg.push(conn);
            }
            _ => {
                unsafe { ManuallyDrop::drop(&mut self.inner) };
            }
        }
    }
}

impl<T> core::ops::Deref for Connection<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> core::ops::DerefMut for Connection<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T> AsRef<T> for Connection<T> {
    fn as_ref(&self) -> &T {
        &self.inner
    }
}

impl<T> AsMut<T> for Connection<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: ConnectionManager> ConnPool<T> {
    pub fn new(manager: T, timeout_duration: std::time::Duration) -> Self {
        Self {
            connections: Arc::new(Queue::new()),
            manager,
            timeout_duration,
        }
    }

    pub async fn get_connection(&self) -> Result<Connection<T::Conn>, ConnectionError<T::Error>> {
        if let Some(conn) = self.connections.pop() {
            return Ok(Connection::new(conn, &self.connections));
        }

        match tokio::time::timeout(self.timeout_duration, self.manager.create_connection()).await {
            Ok(Ok(conn)) => {
                return Ok(Connection::new(conn, &self.connections));
            }
            Ok(Err(e)) => Err(ConnectionError::Creation(e)),
            Err(_) => Err(ConnectionError::Timeout),
        }
    }
}

#[derive(Debug)]
pub enum ConnectionError<E> {
    Timeout,
    Creation(E),
}
