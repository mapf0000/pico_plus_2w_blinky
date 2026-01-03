use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb::driver::{Driver, Endpoint, EndpointError, EndpointIn, EndpointOut};
use embassy_usb::types::InterfaceNumber;
use embassy_usb::{Builder, Handler};
use log::{debug, info, warn};

const MSC_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const BLOCK_SIZE: usize = 512;

const CBW_SIGNATURE: u32 = 0x43425355;
const CSW_SIGNATURE: u32 = 0x53425355;
const CBW_LEN: usize = 31;
const CSW_LEN: usize = 13;
const CBW_FLAG_IN: u8 = 0x80;

const USB_CLASS_MASS_STORAGE: u8 = 0x08;
const MSC_SUBCLASS_SCSI: u8 = 0x06;
const MSC_PROTOCOL_BULK_ONLY: u8 = 0x50;

const SCSI_TEST_UNIT_READY: u8 = 0x00;
const SCSI_REQUEST_SENSE: u8 = 0x03;
const SCSI_INQUIRY: u8 = 0x12;
const SCSI_MODE_SENSE_6: u8 = 0x1A;
const SCSI_START_STOP: u8 = 0x1B;
const SCSI_PREVENT_ALLOW: u8 = 0x1E;
const SCSI_READ_CAPACITY_16: u8 = 0x9E;
const SCSI_READ_FORMAT_CAPACITIES: u8 = 0x23;
const SCSI_READ_CAPACITY_10: u8 = 0x25;
const SCSI_READ_10: u8 = 0x28;
const SCSI_READ_16: u8 = 0x88;
const SCSI_SYNCHRONIZE_CACHE_10: u8 = 0x35;
const SCSI_VERIFY_10: u8 = 0x2F;
const SCSI_WRITE_10: u8 = 0x2A;
const SCSI_MODE_SENSE_10: u8 = 0x5A;
const SCSI_SERVICE_ACTION_READ_CAPACITY_16: u8 = 0x10;

const CSW_STATUS_PASS: u8 = 0x00;
const CSW_STATUS_FAIL: u8 = 0x01;
const CSW_STATUS_PHASE_ERROR: u8 = 0x02;

#[unsafe(link_section = ".msc_image")]
#[used]
static MSC_IMAGE: [u8; MSC_IMAGE_BYTES] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/host-agent.img"));

pub fn image() -> &'static [u8] {
    &MSC_IMAGE
}

pub struct State<'a> {
    control: MaybeUninit<Control<'a>>,
    shared: ControlShared,
}

impl<'a> State<'a> {
    pub const fn new() -> Self {
        Self {
            control: MaybeUninit::uninit(),
            shared: ControlShared::new(),
        }
    }
}

struct ControlShared {
    reset: AtomicBool,
}

impl ControlShared {
    const fn new() -> Self {
        Self {
            reset: AtomicBool::new(false),
        }
    }

    fn take_reset(&self) -> bool {
        self.reset.swap(false, Ordering::Relaxed)
    }
}

struct Control<'a> {
    iface: InterfaceNumber,
    shared: &'a ControlShared,
}

impl<'a> Handler for Control<'a> {
    fn control_out(&mut self, req: Request, data: &[u8]) -> Option<OutResponse> {
        if (req.request_type, req.recipient, req.index)
            != (RequestType::Class, Recipient::Interface, self.iface.0 as u16)
        {
            return None;
        }

        match req.request {
            0xFF if data.is_empty() => {
                self.shared.reset.store(true, Ordering::Relaxed);
                Some(OutResponse::Accepted)
            }
            _ => Some(OutResponse::Rejected),
        }
    }

    fn control_in<'b>(&'b mut self, req: Request, buf: &'b mut [u8]) -> Option<InResponse<'b>> {
        if (req.request_type, req.recipient, req.index)
            != (RequestType::Class, Recipient::Interface, self.iface.0 as u16)
        {
            return None;
        }

        match req.request {
            0xFE => {
                if !buf.is_empty() {
                    buf[0] = 0;
                    Some(InResponse::Accepted(&buf[..1]))
                } else {
                    Some(InResponse::Rejected)
                }
            }
            _ => Some(InResponse::Rejected),
        }
    }
}

pub struct MscClass<'d, D: Driver<'d>> {
    read_ep: D::EndpointOut,
    write_ep: D::EndpointIn,
    max_packet_size: u16,
    shared: &'d ControlShared,
}

impl<'d, D: Driver<'d>> MscClass<'d, D> {
    pub fn new(builder: &mut Builder<'d, D>, state: &'d mut State<'d>, max_packet_size: u16) -> Self {
        let iface_string = builder.string();
        let mut function = builder.function(USB_CLASS_MASS_STORAGE, MSC_SUBCLASS_SCSI, MSC_PROTOCOL_BULK_ONLY);
        let mut interface = function.interface();
        let iface_number = interface.interface_number();
        let mut alt = interface.alt_setting(
            USB_CLASS_MASS_STORAGE,
            MSC_SUBCLASS_SCSI,
            MSC_PROTOCOL_BULK_ONLY,
            Some(iface_string),
        );

        let read_ep = alt.endpoint_bulk_out(None, max_packet_size);
        let write_ep = alt.endpoint_bulk_in(None, max_packet_size);
        drop(function);

        builder.handler(state.control.write(Control {
            iface: iface_number,
            shared: &state.shared,
        }));

        Self {
            read_ep,
            write_ep,
            max_packet_size,
            shared: &state.shared,
        }
    }

    pub async fn run(&mut self, image: &'static [u8]) {
        if image.len() % BLOCK_SIZE != 0 {
            warn!(
                "usb: MSC image length {} is not aligned to block size {}",
                image.len(),
                BLOCK_SIZE
            );
        }

        let block_count = image.len() / BLOCK_SIZE;

        loop {
            self.read_ep.wait_enabled().await;
            self.write_ep.wait_enabled().await;
            info!("usb: MSC ready ({} blocks)", block_count);

            let mut sense = Sense::no_sense();
            loop {
                if self.shared.take_reset() {
                    sense = Sense::no_sense();
                }

                let cbw = match self.read_cbw().await {
                    Ok(Some(cbw)) => cbw,
                    Ok(None) => {
                        sense = Sense::illegal_request();
                        warn!("usb: MSC invalid CBW signature");
                        let _ = self.send_csw(0, 0, CSW_STATUS_PHASE_ERROR).await;
                        continue;
                    }
                    Err(EndpointError::Disabled) => break,
                    Err(err) => {
                        warn!("usb: MSC CBW read error: {:?}", err);
                        continue;
                    }
                };

                let mut data_in = 0usize;
                let mut data_out = 0usize;
                let mut status = CSW_STATUS_PASS;

                match cbw.cmd[0] {
                    SCSI_INQUIRY => {
                        let evpd = cbw.cmd[1] & 0x01 != 0;
                        let page_code = cbw.cmd[2];
                        let alloc_len = cbw.cmd[4] as usize;
                        let data = if evpd {
                            inquiry_vpd_response(page_code)
                        } else {
                            Some(inquiry_response())
                        };
                        if let Some(data) = data {
                            let send_len =
                                core::cmp::min(core::cmp::min(data.len(), alloc_len), cbw.data_len as usize);
                            if cbw.direction_in() && send_len > 0 {
                                data_in = send_len;
                                if let Err(err) = self.write_data(&data[..send_len]).await {
                                    status = CSW_STATUS_PHASE_ERROR;
                                    warn!("usb: MSC inquiry write error: {:?}", err);
                                }
                            }
                        } else {
                            status = CSW_STATUS_FAIL;
                            sense = Sense::illegal_request();
                        }
                    }
                    SCSI_TEST_UNIT_READY => {}
                    SCSI_REQUEST_SENSE => {
                        let data = sense.as_bytes();
                        let send_len = core::cmp::min(data.len(), cbw.data_len as usize);
                        if cbw.direction_in() && send_len > 0 {
                            data_in = send_len;
                            if let Err(err) = self.write_data(&data[..send_len]).await {
                                status = CSW_STATUS_PHASE_ERROR;
                                warn!("usb: MSC request sense write error: {:?}", err);
                            }
                        }
                        sense = Sense::no_sense();
                    }
                    SCSI_READ_CAPACITY_10 => {
                        let data = read_capacity_response(block_count);
                        let send_len = core::cmp::min(data.len(), cbw.data_len as usize);
                        if cbw.direction_in() && send_len > 0 {
                            data_in = send_len;
                            if let Err(err) = self.write_data(&data[..send_len]).await {
                                status = CSW_STATUS_PHASE_ERROR;
                                warn!("usb: MSC read capacity write error: {:?}", err);
                            }
                        }
                    }
                    SCSI_READ_CAPACITY_16 => {
                        let service_action = cbw.cmd[1] & 0x1F;
                        if service_action == SCSI_SERVICE_ACTION_READ_CAPACITY_16 {
                            let data = read_capacity_16_response(block_count);
                            let send_len = core::cmp::min(data.len(), cbw.data_len as usize);
                            if cbw.direction_in() && send_len > 0 {
                                data_in = send_len;
                                if let Err(err) = self.write_data(&data[..send_len]).await {
                                    status = CSW_STATUS_PHASE_ERROR;
                                    warn!("usb: MSC read capacity(16) write error: {:?}", err);
                                }
                            }
                        } else {
                            status = CSW_STATUS_FAIL;
                            sense = Sense::illegal_request();
                        }
                    }
                    SCSI_READ_FORMAT_CAPACITIES => {
                        let data = read_format_capacities_response(block_count);
                        let send_len = core::cmp::min(data.len(), cbw.data_len as usize);
                        if cbw.direction_in() && send_len > 0 {
                            data_in = send_len;
                            if let Err(err) = self.write_data(&data[..send_len]).await {
                                status = CSW_STATUS_PHASE_ERROR;
                                warn!("usb: MSC read format write error: {:?}", err);
                            }
                        }
                    }
                    SCSI_MODE_SENSE_6 => {
                        let data = mode_sense_6_response();
                        let send_len = core::cmp::min(data.len(), cbw.data_len as usize);
                        if cbw.direction_in() && send_len > 0 {
                            data_in = send_len;
                            if let Err(err) = self.write_data(&data[..send_len]).await {
                                status = CSW_STATUS_PHASE_ERROR;
                                warn!("usb: MSC mode sense(6) write error: {:?}", err);
                            }
                        }
                    }
                    SCSI_MODE_SENSE_10 => {
                        let data = mode_sense_10_response();
                        let send_len = core::cmp::min(data.len(), cbw.data_len as usize);
                        if cbw.direction_in() && send_len > 0 {
                            data_in = send_len;
                            if let Err(err) = self.write_data(&data[..send_len]).await {
                                status = CSW_STATUS_PHASE_ERROR;
                                warn!("usb: MSC mode sense(10) write error: {:?}", err);
                            }
                        }
                    }
                    SCSI_READ_10 => {
                        if cbw.direction_in() {
                            let lba = u32::from_be_bytes(cbw.cmd[2..6].try_into().unwrap()) as usize;
                            let blocks = u16::from_be_bytes(cbw.cmd[7..9].try_into().unwrap()) as usize;
                            let byte_len = blocks.saturating_mul(BLOCK_SIZE);
                            let offset = lba.saturating_mul(BLOCK_SIZE);
                            if offset + byte_len <= image.len() {
                                let send_len = core::cmp::min(byte_len, cbw.data_len as usize);
                                if send_len > 0 {
                                    data_in = send_len;
                                    if let Err(err) =
                                        self.write_data(&image[offset..offset + send_len]).await
                                    {
                                        status = CSW_STATUS_PHASE_ERROR;
                                        warn!("usb: MSC read(10) write error: {:?}", err);
                                    }
                                }
                            } else {
                                status = CSW_STATUS_FAIL;
                                sense = Sense::lba_out_of_range();
                            }
                        } else {
                            status = CSW_STATUS_FAIL;
                            sense = Sense::illegal_request();
                        }
                    }
                    SCSI_READ_16 => {
                        if cbw.direction_in() {
                            let lba = u64::from_be_bytes(cbw.cmd[2..10].try_into().unwrap());
                            let blocks = u32::from_be_bytes(cbw.cmd[10..14].try_into().unwrap()) as u64;
                            let byte_len = blocks.saturating_mul(BLOCK_SIZE as u64);
                            let offset = lba.saturating_mul(BLOCK_SIZE as u64);
                            if offset + byte_len <= image.len() as u64 {
                                let send_len = core::cmp::min(byte_len, cbw.data_len as u64) as usize;
                                if send_len > 0 {
                                    data_in = send_len;
                                    let start = offset as usize;
                                    if let Err(err) =
                                        self.write_data(&image[start..start + send_len]).await
                                    {
                                        status = CSW_STATUS_PHASE_ERROR;
                                        warn!("usb: MSC read(16) write error: {:?}", err);
                                    }
                                }
                            } else {
                                status = CSW_STATUS_FAIL;
                                sense = Sense::lba_out_of_range();
                            }
                        } else {
                            status = CSW_STATUS_FAIL;
                            sense = Sense::illegal_request();
                        }
                    }
                    SCSI_WRITE_10 => {
                        status = CSW_STATUS_FAIL;
                        sense = Sense::data_protect();
                    }
                    SCSI_SYNCHRONIZE_CACHE_10 | SCSI_VERIFY_10 => {}
                    SCSI_START_STOP | SCSI_PREVENT_ALLOW => {}
                    _ => {
                        debug!("usb: MSC unhandled SCSI command 0x{:02x}", cbw.cmd[0]);
                        status = CSW_STATUS_FAIL;
                        sense = Sense::illegal_request();
                    }
                }

                if cbw.direction_out() && cbw.data_len > 0 {
                    let remaining = cbw.data_len as usize;
                    if let Err(err) = self.drain_out(remaining).await {
                        warn!("usb: MSC drain error: {:?}", err);
                        status = CSW_STATUS_PHASE_ERROR;
                    } else {
                        data_out = remaining;
                    }
                }

                let residue = cbw
                    .data_len
                    .saturating_sub((data_in + data_out) as u32);
                if status != CSW_STATUS_PASS || residue > 0 {
                    warn!(
                        "usb: MSC csw status=0x{:02x} residue={} sense={:02x}/{:02x}",
                        status, residue, sense.key, sense.asc
                    );
                }
                if let Err(err) = self.send_csw(cbw.tag, residue, status).await {
                    warn!("usb: MSC CSW write error: {:?}", err);
                }
            }
        }
    }

    async fn read_cbw(&mut self) -> Result<Option<Cbw>, EndpointError> {
        let mut buf = [0u8; CBW_LEN];
        self.read_exact(&mut buf).await?;
        Ok(parse_cbw(&buf))
    }

    async fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<(), EndpointError> {
        while !buf.is_empty() {
            let read = self.read_ep.read(buf).await?;
            let tmp = buf;
            buf = &mut tmp[read..];
        }
        Ok(())
    }

    async fn write_data(&mut self, data: &[u8]) -> Result<(), EndpointError> {
        let max_packet = self.max_packet_size as usize;
        for chunk in data.chunks(max_packet) {
            self.write_ep.write(chunk).await?;
        }
        Ok(())
    }

    async fn drain_out(&mut self, mut remaining: usize) -> Result<(), EndpointError> {
        let mut scratch = [0u8; 64];
        while remaining > 0 {
            let chunk = core::cmp::min(remaining, scratch.len());
            let read = self.read_ep.read(&mut scratch[..chunk]).await?;
            remaining = remaining.saturating_sub(read);
        }
        Ok(())
    }

    async fn send_csw(&mut self, tag: u32, residue: u32, status: u8) -> Result<(), EndpointError> {
        let mut buf = [0u8; CSW_LEN];
        buf[..4].copy_from_slice(&CSW_SIGNATURE.to_le_bytes());
        buf[4..8].copy_from_slice(&tag.to_le_bytes());
        buf[8..12].copy_from_slice(&residue.to_le_bytes());
        buf[12] = status;
        self.write_data(&buf).await
    }
}

#[derive(Clone, Copy)]
struct Cbw {
    tag: u32,
    data_len: u32,
    flags: u8,
    cmd: [u8; 16],
}

impl Cbw {
    fn direction_in(&self) -> bool {
        self.flags & CBW_FLAG_IN != 0
    }

    fn direction_out(&self) -> bool {
        !self.direction_in()
    }
}

fn parse_cbw(buf: &[u8; CBW_LEN]) -> Option<Cbw> {
    let signature = u32::from_le_bytes(buf[0..4].try_into().ok()?);
    if signature != CBW_SIGNATURE {
        return None;
    }

    let tag = u32::from_le_bytes(buf[4..8].try_into().ok()?);
    let data_len = u32::from_le_bytes(buf[8..12].try_into().ok()?);
    let flags = buf[12];
    let cmd_len = buf[14];
    if cmd_len == 0 || cmd_len > 16 {
        return None;
    }

    let mut cmd = [0u8; 16];
    cmd.copy_from_slice(&buf[15..31]);

    Some(Cbw {
        tag,
        data_len,
        flags,
        cmd,
    })
}


#[derive(Clone, Copy)]
struct Sense {
    key: u8,
    asc: u8,
    ascq: u8,
}

impl Sense {
    const fn no_sense() -> Self {
        Self {
            key: 0x00,
            asc: 0x00,
            ascq: 0x00,
        }
    }

    const fn illegal_request() -> Self {
        Self {
            key: 0x05,
            asc: 0x20,
            ascq: 0x00,
        }
    }

    const fn lba_out_of_range() -> Self {
        Self {
            key: 0x05,
            asc: 0x21,
            ascq: 0x00,
        }
    }

    const fn data_protect() -> Self {
        Self {
            key: 0x07,
            asc: 0x27,
            ascq: 0x00,
        }
    }

    fn as_bytes(&self) -> [u8; 18] {
        let mut buf = [0u8; 18];
        buf[0] = 0x70;
        buf[2] = self.key;
        buf[7] = 0x0a;
        buf[12] = self.asc;
        buf[13] = self.ascq;
        buf
    }
}

fn inquiry_response() -> &'static [u8] {
    const DATA: [u8; 36] = *b"\x00\x80\x04\x02\x1f\0\0\0PICO    HOST AGENT      1.0 ";
    &DATA
}

fn inquiry_vpd_response(page_code: u8) -> Option<&'static [u8]> {
    const VPD_SUPPORTED: [u8; 7] = [0x00, 0x00, 0x00, 0x03, 0x00, 0x80, 0x83];
    const VPD_SERIAL: [u8; 17] = *b"\x00\x80\x00\rPICOHOSTAGENT";
    const VPD_DEVICE_ID: [u8; 36] =
        *b"\x00\x83\x00\x20\x01\x01\x00\x1cPICO    HOST AGENT      1.0 ";

    match page_code {
        0x00 => Some(&VPD_SUPPORTED),
        0x80 => Some(&VPD_SERIAL),
        0x83 => Some(&VPD_DEVICE_ID),
        _ => None,
    }
}

fn read_capacity_response(block_count: usize) -> [u8; 8] {
    let last_lba = block_count.saturating_sub(1) as u32;
    let mut buf = [0u8; 8];
    buf[..4].copy_from_slice(&last_lba.to_be_bytes());
    buf[4..8].copy_from_slice(&(BLOCK_SIZE as u32).to_be_bytes());
    buf
}

fn read_capacity_16_response(block_count: usize) -> [u8; 32] {
    let last_lba = (block_count.saturating_sub(1)) as u64;
    let mut buf = [0u8; 32];
    buf[..8].copy_from_slice(&last_lba.to_be_bytes());
    buf[8..12].copy_from_slice(&(BLOCK_SIZE as u32).to_be_bytes());
    buf
}

fn read_format_capacities_response(block_count: usize) -> [u8; 12] {
    let blocks = block_count as u32;
    let mut buf = [0u8; 12];
    buf[3] = 8;
    buf[4..8].copy_from_slice(&blocks.to_be_bytes());
    buf[8] = 0x02;
    let block_len = (BLOCK_SIZE as u32).to_be_bytes();
    buf[9] = block_len[1];
    buf[10] = block_len[2];
    buf[11] = block_len[3];
    buf
}

fn mode_sense_6_response() -> [u8; 4] {
    let mut buf = [0u8; 4];
    buf[0] = 3;
    buf[2] = 0x80;
    buf
}

fn mode_sense_10_response() -> [u8; 8] {
    let mut buf = [0u8; 8];
    buf[1] = 6;
    buf[3] = 0x80;
    buf
}
