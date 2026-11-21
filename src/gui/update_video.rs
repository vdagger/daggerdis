//! Видео в `update`: своя трансляция, маршрут по комнате и чужие кадры.
//!
//! Вынесено из `RadioApp::update` целиком, без изменения логики: публикация
//! своих кадров, выбор маршрута по комнате и приём кадров из сети.

use super::*;

impl RadioApp {
    /// Публикует свои кадры, выбирает маршрут по комнате и забирает чужие
    /// кадры из сети (не больше нескольких за апдейт).
    pub(super) fn update_video(&mut self, ctx: &egui::Context) {
        if let Some(video) = &mut self.video {
            let units = video.poll(ctx);
            if let Some(link) = &self.video_link {
                for unit in &units {
                    link.publish(unit, video.info());
                }
            }
        }

        if let Some(link) = &self.video_link {
            link.set_buffer_ms(
                self.config
                    .video_buffer_ms
                    .unwrap_or(crate::network::video_link::DEFAULT_BUFFER_MS),
            );

            // Своя трансляция возобновляет анонс, если он потерялся:
            // «Выйти из эфира» снимает его, а видео при этом не гаснет.
            if self.video.is_some() && !self.room.is_left() {
                if let Some(dir) = &self.video_directory {
                    if dir.my_gen() == 0 {
                        link.begin_stream();
                    }
                }
            }

            // Маршрут по комнате: живой поток, хост и свои флаги.
            // Топология звездой — издатель отдаёт поток хосту, зрители
            // идут к хосту; показ и свой feed считаются от одного набора.
            let left = self.room.is_left();
            let my_gen = self
                .video_directory
                .as_ref()
                .map(|dir| dir.my_gen())
                .unwrap_or(0);
            let is_host = self.is_host.load(std::sync::atomic::Ordering::Relaxed);
            let live: Vec<(String, u32)> = match (&self.video_directory, left) {
                (Some(dir), false) => dir
                    .live(Instant::now())
                    .into_iter()
                    .filter(|(node, _)| *node != self.my_node_id)
                    .collect(),
                _ => Vec::new(),
            };
            let host = if left {
                None
            } else {
                self.room.host_peer().map(|peer| peer.address)
            };
            // Вещающий хост релеем быть не может: его окно занято своим
            // потоком. Живой анонс хоста — признак того, что он вещает.
            let host_busy = host
                .as_deref()
                .map(|h| live.iter().any(|(node, _)| node == h))
                .unwrap_or(false);
            let host_ref = host.as_deref();

            let route = crate::network::video_link::choose_view_route(
                is_host, left, my_gen, &live, host_ref, host_busy,
            );
            link.sync_viewing(route.target());
            let feed_target = crate::network::video_link::choose_feed_target(
                is_host, my_gen, &live, host_ref, host_busy,
            );
            link.sync_feed(feed_target);

            // Источник пропал или сменился — старое окно показывать нечего.
            // Сброс очереди (обрыв, переподключение) тоже гасит картинку:
            // последний кадр прошлой подписки иначе висел бы на экране.
            let stale_peer = match (&self.remote, route.expected_peer()) {
                (Some(remote), Some(peer)) => peer != remote.peer,
                (Some(_), None) => true,
                (None, _) => false,
            };
            let stale_epoch = link.remote_epoch() != self.remote_epoch_seen;
            if stale_peer || stale_epoch {
                self.remote = None;
            }
            self.remote_epoch_seen = link.remote_epoch();
        }

        // чужие кадры из сети; за один апдейт не больше нескольких,
        // чтобы очередь декодирования не растягивала кадр GUI
        if let Some(link) = &self.video_link {
            let mut budget = 4;
            while budget > 0 {
                let Some(frame) = link.try_recv_due() else {
                    break;
                };
                budget -= 1;
                if self.remote.is_none() {
                    let info = crate::video::StreamInfo {
                        width: frame.width,
                        height: frame.height,
                        fps: frame.fps,
                        sequence_header: frame.sequence_header.clone(),
                    };
                    match video_session::RemoteView::new(&info, frame.from.clone()) {
                        Ok(v) => {
                            self.remote = Some(v);
                            // зрителю не нужно искать кнопку — сразу открываем окно
                            self.video_window = true;
                        }
                        Err(e) => self.video_error = Some(e),
                    }
                }
                if let Some(remote) = &mut self.remote {
                    remote.feed(frame.unit, ctx);
                }
                // счётчик для отчёта: сколько кадров реально принято
                self.remote_frames += 1;
            }
        }
        // забрать готовый кадр, даже если новых не пришло
        if let Some(remote) = &mut self.remote {
            remote.tick(ctx);
        }
        // забрать готовый кадр, даже если новых не пришло
        if let Some(remote) = &mut self.remote {
            remote.tick(ctx);
        }
    }
}
