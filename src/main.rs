mod utils;

use crate::utils::{is_timed_out, timestamp, TIMEOUT_SECOND};
use ppp::v2;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::env;
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio::{self, select};
use tracing::log::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::fmt::init();

	let args: Vec<String> = env::args().collect();
	let bind_addr = &*args[1];
	let bind_port =  args[2].parse::<u16>()?;
	let bind_addr_upstream = SocketAddr::from_str(&*format!("{}:0", bind_addr))?;
	let remote_addr = (&*args[3], args[4].parse::<u16>()?).to_socket_addrs()?.next().unwrap();

	let local_socket = UdpSocket::bind(format!("{}:{}", bind_addr, bind_port)).await?;

	let mut buf = [0; 64 * 1024];
	let (send, mut recv) = unbounded_channel::<(SocketAddr, Vec<u8>)>();
	let send_lock = Arc::new(Mutex::new(send));
	let socket_addr_map: Arc<Mutex<HashMap<SocketAddr, (Arc<UdpSocket>, i64)>>> = Arc::new(Mutex::new(HashMap::new()));

	loop {
		select! {
			recv = local_socket.recv_from(&mut buf) => {
				let mut socket_addr_map_lock = socket_addr_map.lock().await;
				let (size, src_addr) = recv?;
				let mut old_stream = false;
				let upstream: Arc<UdpSocket>;

				info!("recv from {}:{} (size: {})", src_addr.ip().to_string(), src_addr.port(), size);

				if let Entry::Vacant(e) = socket_addr_map_lock.entry(src_addr) {
					upstream = Arc::new(UdpSocket::bind(bind_addr_upstream).await?);
					e.insert((upstream.clone(), timestamp()));

					info!("bind forwarding address {}:{}", upstream.local_addr()?.ip().to_string(), upstream.local_addr()?.port());
				} else {
					upstream = socket_addr_map_lock[&src_addr].0.clone();
					socket_addr_map_lock.get_mut(&src_addr).unwrap().1 = timestamp();
					old_stream = true;
				}

				info!("send to upstream {} (size: {})", remote_addr, size);

				let mut pp_buf = v2::Builder::with_addresses(
					v2::Version::Two | v2::Command::Proxy,
					v2::Protocol::Datagram,
					(src_addr, remote_addr),
				).build()?;
				pp_buf.append(&mut buf[..size].to_vec());

				upstream.send_to(pp_buf.as_slice(), &remote_addr).await?;

				if !old_stream {
					let send_lck = send_lock.clone();
					let socket_addr_map_in_worker_lck = socket_addr_map.clone();
					tokio::spawn(async move {
						let mut buf = [0; 64 * 1024];
						loop{
							match timeout(Duration::from_secs(TIMEOUT_SECOND),upstream.recv_from(&mut buf)).await{
								Ok(p) => {
									let size = p.unwrap().0;
									info!("send to downstream {}:{} (size: {})", src_addr.ip().to_string(), src_addr.port(), size);
									send_lck.lock().await.send((src_addr, buf[..size].to_vec())).unwrap();
								},
								Err(_) => {
									let mut socket_addr_map = socket_addr_map_in_worker_lck.lock().await;
									if is_timed_out(socket_addr_map[&src_addr].1, TIMEOUT_SECOND){
										info!("unbind {}:{} for source address {}:{}", socket_addr_map[&src_addr].0.local_addr().unwrap().ip().to_string(), socket_addr_map[&src_addr].0.local_addr().unwrap().port(), src_addr.ip().to_string(), src_addr.port());
										socket_addr_map.remove(&src_addr);
										break;
									}
								}
							};
						}
					});
				}
			},
			send = recv.recv() => {
				let (src_addr, data) = send.unwrap();
				local_socket.send_to(data.as_slice(), src_addr).await?;
			}
		}
	}
}
