//! The live TV behind a session: pairing credentials, pinned sockets, and capture.
use crate::setup;
use std::{net::SocketAddr, time::Duration};
use webos_mcp::{
    capture::{CaptureConfig, CapturedImage, RecoveringCapture},
    error::{TvError, outcome},
    metrics::{Metrics, Operation, Outcome, Stage},
    session::{Action, Device},
    transport::{self, AuthenticatedSocket},
};

/// The real TV: pinned control and input sockets plus optional SSH capture.
pub struct LiveDevice {
    remote: Option<AuthenticatedSocket>,
    input: Option<AuthenticatedSocket>,
    capture: Option<RecoveringCapture>,
    cleanup_unconfirmed: bool,
}
impl LiveDevice {
    /// A device that owns no connection yet.
    pub fn idle() -> Self {
        Self {
            remote: None,
            input: None,
            capture: None,
            cleanup_unconfirmed: false,
        }
    }
    async fn connect_control(
        &mut self,
        endpoint: SocketAddr,
        fingerprint: [u8; 32],
        metrics: &Metrics,
    ) -> Result<(), TvError> {
        self.cleanup_unconfirmed = true;
        let remote = transport::connect(endpoint, fingerprint, metrics).await;
        // A completed failed handshake drops its local socket. Cancellation does
        // not reach this point and must retain uncertainty.
        self.cleanup_unconfirmed = false;
        self.remote = Some(remote?);
        Ok(())
    }
}
impl Device for LiveDevice {
    async fn establish(&mut self, metrics: &Metrics) -> Result<bool, TvError> {
        let measurement = metrics.stage(Operation::Connect, Stage::CredentialLookup);
        let loaded = setup::load_device_credentials();
        measurement.finish(outcome(&loaded));
        let (address, mut credentials) = loaded?;
        let endpoint = SocketAddr::new(address, 3001);
        let measurement = metrics.stage(Operation::Connect, Stage::ConnectionSetup);
        let remote = self
            .connect_control(endpoint, credentials.fingerprint, metrics)
            .await;
        measurement.finish(outcome(&remote));
        remote?;
        let measurement = metrics.stage(Operation::Connect, Stage::Registration);
        let key =
            transport::register(self.remote.as_mut().unwrap(), Some(&credentials.client_key)).await;
        measurement.finish(outcome(&key));
        setup::persist_verified_credentials(&mut credentials, address, key?)?;
        self.cleanup_unconfirmed = true;
        let measurement = metrics.stage(Operation::Connect, Stage::ConnectionSetup);
        let input = self
            .remote
            .as_mut()
            .unwrap()
            .input_socket(endpoint, metrics)
            .await;
        measurement.finish(outcome(&input));
        self.cleanup_unconfirmed = false;
        self.input = Some(input?);
        if let Some(config) = credentials.ssh.take() {
            self.cleanup_unconfirmed = true;
            let capture = RecoveringCapture::connect(CaptureConfig {
                endpoint: SocketAddr::new(address, config.port),
                username: config.username,
                agent_socket: config.agent_socket,
                public_key: config.public_key,
            })
            .await;
            if let Err(error) = &capture
                && !error.cleanup_unconfirmed()
            {
                self.cleanup_unconfirmed = false;
            }
            self.capture = Some(capture?);
            self.cleanup_unconfirmed = false;
        }
        Ok(self.capture.is_some())
    }
    async fn probe(&mut self) -> Result<(), TvError> {
        let input = self.input.as_mut().ok_or("TV input is unavailable")?;
        let remote = self.remote.as_mut().ok_or("TV control is unavailable")?;
        // The sockets are independent; check both in one round trip.
        let (input, remote) = tokio::join!(input.probe(), remote.probe());
        input?;
        remote
    }
    async fn send(&mut self, action: &Action) -> Result<(), TvError> {
        match action {
            Action::Press { button, count } => {
                for i in 0..*count {
                    if i > 0 {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                    }
                    self.input
                        .as_mut()
                        .ok_or("TV input is unavailable")?
                        .send_button(button)
                        .await?;
                }
                Ok(())
            }
            Action::Launch { app_id } => {
                self.remote
                    .as_mut()
                    .ok_or("TV control is unavailable")?
                    .launch(app_id)
                    .await
            }
        }
    }
    async fn capture(&mut self) -> Result<CapturedImage, TvError> {
        self.capture
            .as_mut()
            .ok_or("Visual capture is unavailable")?
            .capture()
            .await
            .map_err(TvError::from)
    }
    async fn capture_with_metrics(
        &mut self,
        op: Operation,
        metrics: &Metrics,
    ) -> Result<CapturedImage, TvError> {
        self.capture
            .as_mut()
            .ok_or("Visual capture is unavailable")?
            .capture_with_metrics(op, metrics)
            .await
            .map_err(TvError::from)
    }
    async fn close(&mut self) -> Result<(), TvError> {
        let prior_uncertainty = self.cleanup_unconfirmed;
        self.cleanup_unconfirmed = true;
        let input = match self.input.take() {
            Some(s) => s.close().await,
            None => Ok(()),
        };
        let remote = match self.remote.take() {
            Some(s) => s.close().await,
            None => Ok(()),
        };
        let capture = match self.capture.as_mut() {
            Some(s) => s.close().await.map_err(TvError::from),
            None => Ok(()),
        };
        if prior_uncertainty || input.is_err() || remote.is_err() || capture.is_err() {
            // The sockets are gone and the flag never clears, so only a new
            // process can release this session. Say so instead of implying a
            // later disconnect could succeed.
            let message =
                "TV cleanup could not be confirmed; restart the MCP server before reconnecting";
            let timed_out = [input, remote, capture]
                .iter()
                .any(|closed| outcome(closed) == Outcome::Timeout);
            return Err(if timed_out {
                TvError::timeout(message)
            } else {
                TvError::failure(message)
            });
        }
        self.cleanup_unconfirmed = false;
        Ok(())
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
