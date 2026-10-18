// SPDX-License-Identifier: GPL-3.0-or-later
//! GTK3 window plus the thread-safe `UiHandle` used by the async transports.
#![allow(deprecated)] // glib::MainContext::channel is the simplest cross-thread bridge in gtk-rs 0.18

use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gdk_pixbuf, glib};
use tokio::sync::{mpsc, Mutex, Notify};

use crate::protocol::HelperError;

#[derive(Debug)]
enum UiMsg {
    Qr(String),
    NoPhone,
    Status(String),
    PinPrompt(String),
    PinHide,
}

/// Cheap, cloneable, `Send` handle. With no window (`headless`) all UI calls are no-ops and
/// PIN prompts fail.
#[derive(Clone)]
pub struct UiHandle {
    tx: Option<glib::Sender<UiMsg>>,
    pin_rx: Arc<Mutex<mpsc::UnboundedReceiver<Option<String>>>>,
    /// Notified when the user cancels (button / window close).
    pub cancel: Arc<Notify>,
}

impl UiHandle {
    pub fn headless() -> Self {
        let (_tx, rx) = mpsc::unbounded_channel();
        UiHandle {
            tx: None,
            pin_rx: Arc::new(Mutex::new(rx)),
            cancel: Arc::new(Notify::new()),
        }
    }

    fn send(&self, m: UiMsg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(m);
        }
    }

    pub fn show_qr(&self, payload: &str) {
        self.send(UiMsg::Qr(payload.to_string()));
    }

    pub fn no_phone(&self) {
        self.send(UiMsg::NoPhone);
    }

    pub fn status(&self, text: &str) {
        tracing::info!("status: {text}");
        self.send(UiMsg::Status(text.to_string()));
    }

    /// Asks the user for a PIN. `prefix` is an optional message such as "Incorrect PIN".
    pub async fn ask_pin(
        &self,
        attempts_left: Option<u32>,
        prefix: Option<&str>,
    ) -> Result<String, HelperError> {
        if self.tx.is_none() {
            return Err(HelperError::Ctap(
                0x36,
                "PIN required but no display is available to ask for it".into(),
            ));
        }
        let mut text = String::new();
        if let Some(p) = prefix {
            text.push_str(p);
            text.push_str(". ");
        }
        text.push_str("Enter your security key PIN");
        if let Some(n) = attempts_left {
            text.push_str(&format!(" ({n} attempts left)"));
        }
        let mut rx = self.pin_rx.lock().await;
        while rx.try_recv().is_ok() {}
        self.send(UiMsg::PinPrompt(text));
        let res = rx.recv().await;
        self.send(UiMsg::PinHide);
        match res {
            Some(Some(pin)) if !pin.is_empty() => Ok(pin),
            _ => Err(HelperError::Cancelled),
        }
    }
}

fn qr_pixbuf(payload: &str) -> Option<gdk_pixbuf::Pixbuf> {
    let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
    let n = code.width();
    let quiet = 4usize;
    let total = n + 2 * quiet;
    let scale = (320 / total).max(2);
    let px = total * scale;
    let colors = code.to_colors();
    let mut data = vec![255u8; px * px * 3];
    for y in 0..n {
        for x in 0..n {
            if colors[y * n + x] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let ix = ((quiet + y) * scale + dy) * px + (quiet + x) * scale + dx;
                        data[ix * 3..ix * 3 + 3].fill(0);
                    }
                }
            }
        }
    }
    Some(gdk_pixbuf::Pixbuf::from_bytes(
        &glib::Bytes::from_owned(data),
        gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        px as i32,
        px as i32,
        (px * 3) as i32,
    ))
}

/// Builds and shows the window. Must be called on the GTK main thread after `gtk::init()`.
pub fn create(rp_id: &str) -> UiHandle {
    let (tx, rx) = glib::MainContext::channel::<UiMsg>(glib::Priority::DEFAULT);
    let (pin_tx, pin_rx) = mpsc::unbounded_channel::<Option<String>>();
    let cancel = Arc::new(Notify::new());

    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title(&format!("Sign in with your passkey \u{2014} {rp_id}"));
    win.set_position(gtk::WindowPosition::Center);
    win.set_resizable(false);
    win.set_keep_above(true);
    win.set_border_width(16);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let scan_label = gtk::Label::new(Some("Scan with your phone's camera (Android / iPhone)"));
    let qr_image = gtk::Image::new();
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    let key_label = gtk::Label::new(Some("\u{2026}or touch your security key"));
    let status = gtk::Label::new(Some("Waiting for phone or security key"));
    status.set_line_wrap(true);
    status.set_max_width_chars(48);

    let pin_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let pin_label = gtk::Label::new(Some("Enter your security key PIN"));
    pin_label.set_line_wrap(true);
    pin_label.set_max_width_chars(48);
    let pin_entry = gtk::Entry::new();
    pin_entry.set_visibility(false);
    pin_entry.set_input_purpose(gtk::InputPurpose::Pin);
    pin_entry.set_hexpand(true);
    let pin_ok = gtk::Button::with_label("OK");
    pin_row.pack_start(&pin_entry, true, true, 0);
    pin_row.pack_start(&pin_ok, false, false, 0);
    let pin_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    pin_box.pack_start(&pin_label, false, false, 0);
    pin_box.pack_start(&pin_row, false, false, 0);

    let cancel_btn = gtk::Button::with_label("Cancel");

    vbox.pack_start(&scan_label, false, false, 0);
    vbox.pack_start(&qr_image, false, false, 0);
    vbox.pack_start(&sep, false, false, 4);
    vbox.pack_start(&key_label, false, false, 0);
    vbox.pack_start(&status, false, false, 0);
    vbox.pack_start(&pin_box, false, false, 0);
    vbox.pack_start(&cancel_btn, false, false, 4);
    win.add(&vbox);
    win.show_all();
    pin_box.hide();
    qr_image.hide();
    scan_label.set_text("Preparing phone sign-in\u{2026}");

    // Input -> async world.
    {
        let cancel = cancel.clone();
        let pin_tx = pin_tx.clone();
        let do_cancel = move || {
            cancel.notify_one();
            let _ = pin_tx.send(None);
        };
        let c1 = do_cancel.clone();
        cancel_btn.connect_clicked(move |_| c1());
        win.connect_delete_event(move |_, _| {
            do_cancel();
            glib::Propagation::Stop
        });
    }
    {
        let submit = {
            let pin_tx = pin_tx.clone();
            let entry = pin_entry.clone();
            move || {
                let text: String = entry.text().into();
                entry.set_text("");
                let _ = pin_tx.send(Some(text));
            }
        };
        let s1 = submit.clone();
        pin_entry.connect_activate(move |_| s1());
        pin_ok.connect_clicked(move |_| submit());
    }

    // Async world -> UI.
    {
        let win = win.clone();
        rx.attach(None, move |msg| {
            match msg {
                UiMsg::Qr(payload) => match qr_pixbuf(&payload) {
                    Some(pb) => {
                        qr_image.set_from_pixbuf(Some(&pb));
                        qr_image.show();
                        scan_label.set_text("Scan with your phone's camera (Android / iPhone)");
                    }
                    None => scan_label.set_text("Could not render QR code"),
                },
                UiMsg::NoPhone => {
                    qr_image.hide();
                    scan_label.set_text("Phone sign-in is not available on this machine");
                }
                UiMsg::Status(s) => status.set_text(&s),
                UiMsg::PinPrompt(t) => {
                    pin_label.set_text(&t);
                    pin_box.show_all();
                    pin_entry.grab_focus();
                    win.present();
                }
                UiMsg::PinHide => pin_box.hide(),
            }
            glib::ControlFlow::Continue
        });
    }
    win.present();

    UiHandle {
        tx: Some(tx),
        pin_rx: Arc::new(Mutex::new(pin_rx)),
        cancel,
    }
}
