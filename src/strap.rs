use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use btleplug::api::{
    Central as _, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures::{Stream, StreamExt as _};
use uuid::{Uuid, uuid};

use crate::frame::{Command, Frame, Reassembler};

const SERVICE: Uuid = uuid!("61080001-8d6d-82b8-614a-1c8cb0f8dcc6");
const COMMAND_WRITE: Uuid = uuid!("61080002-8d6d-82b8-614a-1c8cb0f8dcc6");
const NOTIFY: [Uuid; 3] = [
    uuid!("61080003-8d6d-82b8-614a-1c8cb0f8dcc6"),
    uuid!("61080004-8d6d-82b8-614a-1c8cb0f8dcc6"),
    uuid!("61080005-8d6d-82b8-614a-1c8cb0f8dcc6"),
];
const SCAN_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Strap {
    peripheral: Peripheral,
    command: Characteristic,
    seq: u8,
    pub name: String,
}

impl Strap {
    pub async fn connect() -> Result<Self> {
        let manager = Manager::new().await?;
        let adapter = manager
            .adapters()
            .await?
            .into_iter()
            .next()
            .context("no bluetooth adapter")?;
        adapter
            .start_scan(ScanFilter {
                services: vec![SERVICE],
            })
            .await?;
        let (peripheral, name) = tokio::time::timeout(SCAN_TIMEOUT, async {
            loop {
                for p in adapter.peripherals().await? {
                    if let Some(props) = p.properties().await?
                        && props.services.contains(&SERVICE)
                    {
                        return anyhow::Ok((p, props.local_name.unwrap_or_default()));
                    }
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
        .await
        .context("no WHOOP 4 strap advertising; is the phone's bluetooth off?")??;
        adapter.stop_scan().await?;

        peripheral.connect().await.context("connect")?;
        peripheral
            .discover_services()
            .await
            .context("discover services")?;
        let chars = peripheral.characteristics();
        let command = chars
            .iter()
            .find(|c| c.uuid == COMMAND_WRITE)
            .cloned()
            .context("strap lacks the command characteristic")?;
        for c in chars.iter().filter(|c| NOTIFY.contains(&c.uuid)) {
            peripheral
                .subscribe(c)
                .await
                .with_context(|| format!("subscribe {}", c.uuid))?;
        }
        let mut strap = Self {
            peripheral,
            command,
            seq: 0,
            name,
        };
        strap
            .send(Command::GetBatteryLevel)
            .await
            .context("first confirmed write; is the strap bonded to this machine?")?;
        Ok(strap)
    }

    pub async fn send(&mut self, command: Command) -> Result<()> {
        self.seq = self.seq.wrapping_add(1);
        self.peripheral
            .write(
                &self.command,
                &command.encode(self.seq),
                WriteType::WithResponse,
            )
            .await?;
        Ok(())
    }

    pub async fn frames(&self) -> Result<impl Stream<Item = Frame> + use<>> {
        let notifications = self.peripheral.notifications().await?;
        let mut reassemblers: HashMap<Uuid, Reassembler> = HashMap::new();
        Ok(notifications.flat_map(move |n| {
            let frames = reassemblers.entry(n.uuid).or_default().feed(&n.value);
            futures::stream::iter(frames)
        }))
    }

    pub async fn disconnect(&self) -> Result<()> {
        if !self.peripheral.is_connected().await? {
            bail!("strap already disconnected");
        }
        self.peripheral.disconnect().await?;
        Ok(())
    }
}
