use std::{
    fs::File,
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use bytemuck::{Pod, Zeroable};
use mesocarp::comms::buses::ThreadedMessenger;
use mesocarp::MesoError;

use crate::{objects::Transfer, AikaError};

pub struct MessageBus<const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> {
    pub(crate) messenger: ThreadedMessenger<MSG_BW, Transfer<MessageType>>,
    pub(crate) terminal_flag: Arc<AtomicBool>,
    pub(crate) log: Option<File>,
    pub(crate) start: Instant,
}

impl<const MSG_BW: usize, MessageType: Pod + Zeroable + Clone> MessageBus<MSG_BW, MessageType> {
    pub(crate) fn from_substrate(
        messenger: ThreadedMessenger<MSG_BW, Transfer<MessageType>>,
        log: Option<File>,
        start: Instant,
        terminal_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            messenger,
            terminal_flag,
            log,
            start,
        }
    }

    pub(crate) fn deliver_the_mail(&mut self) -> Result<(), AikaError> {
        match self.messenger.poll() {
            Ok(msgs) => {
                if let Some(file) = &mut self.log {
                    writeln!(
                        file,
                        "[{}] Found {:?} messages in-transit.",
                        self.start.elapsed().as_micros(),
                        msgs.len()
                    )
                    .map_err(|_| AikaError::LoggingWriteError)?;
                }
                self.messenger.deliver(msgs)?;
                Ok(())
            }
            Err(err) => {
                if let MesoError::NoDirectCommsToShare = err {
                    Ok(())
                } else {
                    if let Some(file) = &mut self.log {
                        writeln!(
                            file,
                            "[{}] Error delivering mail!",
                            self.start.elapsed().as_micros(),
                        )
                        .map_err(|_| AikaError::LoggingWriteError)?;
                    }
                    Err(AikaError::MesoError(err))
                }
            }
        }
    }

    pub fn master(mut self) -> Result<Self, AikaError> {
        if let Some(log) = &mut self.log {
            writeln!(
                log,
                "[{}] Starting Master",
                self.start.elapsed().as_micros()
            )
            .map_err(|_| AikaError::LoggingWriteError)?;
        }
        loop {
            let flag = self.terminal_flag.load(Ordering::SeqCst);
            if flag {
                if let Some(log) = &mut self.log {
                    writeln!(
                        log,
                        "[{}] Flag closing Master",
                        self.start.elapsed().as_micros()
                    )
                    .map_err(|_| AikaError::LoggingWriteError)?;
                }
                break;
            }
            if let Some(log) = &mut self.log {
                writeln!(log, "[{}] looping master", self.start.elapsed().as_micros())
                    .map_err(|_| AikaError::LoggingWriteError)?;
            }
            for _ in 0..100 {
                self.deliver_the_mail()?;
            }
            std::thread::sleep(Duration::from_nanos(1));
        }
        if let Some(log) = &mut self.log {
            writeln!(log, "[{}] Closing Master", self.start.elapsed().as_micros())
                .map_err(|_| AikaError::LoggingWriteError)?;
        }
        Ok(self)
    }
}
