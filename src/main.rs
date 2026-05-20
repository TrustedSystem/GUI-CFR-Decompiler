#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // Hide console window on Windows in release builds

use eframe::egui;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use std::thread;
use std::sync::mpsc::{channel, Receiver};

// Embed the CFR Decompiler jar directly into our binary!
const CFR_JAR_BYTES: &[u8] = include_bytes!("../cfr.jar");

/// Messages sent from the background decompiler thread to the GUI thread.
enum DecompileMessage {
    Log(String),
    Success,
    Error(String),
}

/// The current status of the decompilation process.
#[derive(Clone, PartialEq)]
enum DecompileStatus {
    Idle,
    Decompiling,
    Success,
    Error(String),
}

struct DecompilerApp {
    source_path: String,
    dest_path: String,
    status: DecompileStatus,
    log_messages: Vec<String>,
    receiver: Option<Receiver<DecompileMessage>>,
}

impl Default for DecompilerApp {
    fn default() -> Self {
        Self {
            source_path: String::new(),
            dest_path: String::new(),
            status: DecompileStatus::Idle,
            log_messages: Vec::new(),
            receiver: None,
        }
    }
}

impl DecompilerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Set up the custom, ultra-premium dark theme
        let mut visuals = egui::Visuals::dark();
        
        // Window and element roundings
        visuals.window_rounding = 12.0.into();
        visuals.widgets.noninteractive.rounding = 8.0.into();
        visuals.widgets.inactive.rounding = 6.0.into();
        visuals.widgets.hovered.rounding = 6.0.into();
        visuals.widgets.active.rounding = 6.0.into();
        
        // Premium Dark Color Palette (Slate & Charcoal)
        visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(15, 15, 18);
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 30, 38));
        
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(26, 26, 32);
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 45, 56));
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(212, 212, 216));
        
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(38, 38, 48);
        // Glowing violet borders for hovered interactive elements
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.2, egui::Color32::from_rgb(139, 92, 246));
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255));
        
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(49, 49, 64);
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(139, 92, 246));
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255));
        
        // Deep purple selection colors
        visuals.selection.bg_fill = egui::Color32::from_rgb(109, 40, 217);
        visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255));
        
        cc.egui_ctx.set_visuals(visuals);
        
        // Set custom spacing
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        cc.egui_ctx.set_style(style);
        
        Self::default()
    }
    
    /// Extracts the embedded `cfr.jar` to the OS temporary directory so it can be invoked via Java.
    fn extract_cfr_jar(&self) -> Result<PathBuf, std::io::Error> {
        let mut temp_dir = std::env::temp_dir();
        temp_dir.push("cfr_decompiler_wrapper");
        std::fs::create_dir_all(&temp_dir)?;
        
        let jar_path = temp_dir.join("cfr_embedded.jar");
        
        // Write the jar bytes only if file doesn't exist or size is different
        let needs_write = if jar_path.exists() {
            let metadata = std::fs::metadata(&jar_path)?;
            metadata.len() != CFR_JAR_BYTES.len() as u64
        } else {
            true
        };
        
        if needs_write {
            std::fs::write(&jar_path, CFR_JAR_BYTES)?;
        }
        
        Ok(jar_path)
    }

    /// Spawns the background decompiler thread.
    fn start_decompilation(&mut self) {
        self.status = DecompileStatus::Decompiling;
        self.log_messages.clear();
        self.log_messages.push("[SYSTEM] Extracting embedded CFR decompiler...".to_string());
        
        // Extract embedded JAR
        let jar_path = match self.extract_cfr_jar() {
            Ok(path) => path,
            Err(e) => {
                self.status = DecompileStatus::Error(format!("Extraction error: {}", e));
                self.log_messages.push(format!("[ERROR] Failed to extract decompiler JAR: {}", e));
                return;
            }
        };
        
        self.log_messages.push("[SYSTEM] Successfully extracted CFR decompiler.".to_string());
        self.log_messages.push("[SYSTEM] Starting CFR subprocess...".to_string());
        
        let (tx, rx) = channel();
        self.receiver = Some(rx);
        
        let jar_path_str = jar_path.to_string_lossy().to_string();
        let source = self.source_path.clone();
        let dest = self.dest_path.clone();
        
        // Spawn the decompiler worker thread
        thread::spawn(move || {
            let mut cmd = Command::new("java");
            cmd.arg("-jar")
               .arg(&jar_path_str)
               .arg(&source)
               .arg("--outputdir")
               .arg(&dest)
               .stdout(Stdio::piped())
               .stderr(Stdio::piped());
               
            // On Windows, hide the console window of the spawned java process
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(DecompileMessage::Error(format!(
                        "Failed to spawn Java. Is Java installed and in your PATH? Error: {}",
                        e
                    )));
                    return;
                }
            };
            
            // Read stdout in a background channel thread
            let stdout = child.stdout.take().unwrap();
            let tx_stdout = tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if let Ok(l) = line {
                        let _ = tx_stdout.send(DecompileMessage::Log(l));
                    }
                }
            });
            
            // Read stderr in a background channel thread
            let stderr = child.stderr.take().unwrap();
            let tx_stderr = tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    if let Ok(l) = line {
                        let _ = tx_stderr.send(DecompileMessage::Log(format!("[ERROR] {}", l)));
                    }
                }
            });
            
            // Wait for the subprocess to complete
            match child.wait() {
                Ok(status) => {
                    if status.success() {
                        let _ = tx.send(DecompileMessage::Success);
                    } else {
                        let _ = tx.send(DecompileMessage::Error(format!(
                            "CFR Decompiler exited with error code: {}",
                            status
                        )));
                    }
                }
                Err(e) => {
                    let _ = tx.send(DecompileMessage::Error(format!(
                        "Failed to wait for decompiler process: {}",
                        e
                    )));
                }
            }
        });
    }
}

impl eframe::App for DecompilerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle incoming messages from the background thread
        let mut should_clear_receiver = false;
        if let Some(ref rx) = self.receiver {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    DecompileMessage::Log(line) => {
                        self.log_messages.push(line);
                    }
                    DecompileMessage::Success => {
                        self.status = DecompileStatus::Success;
                        self.log_messages.push("[SYSTEM] Decompilation finished successfully!".to_string());
                        should_clear_receiver = true;
                    }
                    DecompileMessage::Error(err) => {
                        self.status = DecompileStatus::Error(err.clone());
                        self.log_messages.push(format!("[ERROR] {}", err));
                        should_clear_receiver = true;
                    }
                }
            }
        }
        if should_clear_receiver {
            self.receiver = None;
        }
        
        // Request repaints continuously while decompiling to update the console in real-time
        if self.status == DecompileStatus::Decompiling {
            ctx.request_repaint();
        }
        
        // Render the main visual panel
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                // Header Title with sleek gradients/colors
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("CFR Java Decompiler")
                            .font(egui::FontId::proportional(22.0))
                            .strong()
                            .color(egui::Color32::from_rgb(255, 255, 255))
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("v0.152 (Embedded)")
                                .font(egui::FontId::monospace(11.0))
                                .color(egui::Color32::from_rgb(139, 92, 246))
                        );
                    });
                });
                
                ui.add(egui::Separator::default().spacing(10.0));
                
                // Form Area
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(22, 22, 27))
                    .rounding(8.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            // Source JAR Selection
                            ui.label(egui::RichText::new("Target Mod File (.jar):").strong().color(egui::Color32::from_rgb(200, 200, 200)));
                            ui.horizontal(|ui| {
                                let disabled = self.status == DecompileStatus::Decompiling;
                                ui.add_enabled_ui(!disabled, |ui| {
                                    let btn = egui::Button::new(
                                        egui::RichText::new("📂 Choose JAR File...")
                                            .strong()
                                    );
                                    if ui.add(btn).clicked() {
                                        if let Some(file) = rfd::FileDialog::new()
                                            .add_filter("JAR File (*.jar)", &["jar"])
                                            .pick_file() 
                                        {
                                            self.source_path = file.to_string_lossy().to_string();
                                        }
                                    }
                                });
                                
                                let display_text = if self.source_path.is_empty() {
                                    "No JAR file selected".to_string()
                                } else {
                                    self.source_path.clone()
                                };
                                ui.label(
                                    egui::RichText::new(display_text)
                                        .color(if self.source_path.is_empty() { egui::Color32::from_rgb(110, 110, 130) } else { egui::Color32::from_rgb(210, 210, 215) })
                                );
                            });
                            
                            ui.add_space(10.0);
                            
                            // Output Folder Selection
                            ui.label(egui::RichText::new("Output Directory:").strong().color(egui::Color32::from_rgb(200, 200, 200)));
                            ui.horizontal(|ui| {
                                let disabled = self.status == DecompileStatus::Decompiling;
                                ui.add_enabled_ui(!disabled, |ui| {
                                    let btn = egui::Button::new(
                                        egui::RichText::new("📁 Choose Output Folder...")
                                            .strong()
                                    );
                                    if ui.add(btn).clicked() {
                                        if let Some(folder) = rfd::FileDialog::new()
                                            .pick_folder() 
                                        {
                                            self.dest_path = folder.to_string_lossy().to_string();
                                        }
                                    }
                                });
                                
                                let display_text = if self.dest_path.is_empty() {
                                    "No output directory selected".to_string()
                                } else {
                                    self.dest_path.clone()
                                };
                                ui.label(
                                    egui::RichText::new(display_text)
                                        .color(if self.dest_path.is_empty() { egui::Color32::from_rgb(110, 110, 130) } else { egui::Color32::from_rgb(210, 210, 215) })
                                );
                            });
                        });
                    });
                
                ui.add_space(6.0);
                
                // Action Buttons and General Status Row
                ui.horizontal(|ui| {
                    let is_decompiling = self.status == DecompileStatus::Decompiling;
                    let has_inputs = !self.source_path.trim().is_empty() && !self.dest_path.trim().is_empty();
                    
                    ui.add_enabled_ui(!is_decompiling && has_inputs, |ui| {
                        // Prominent Decompile button styled beautifully
                        let btn_text = if is_decompiling { "Decompiling..." } else { "Decompile" };
                        let mut btn = egui::Button::new(
                            egui::RichText::new(btn_text)
                                .font(egui::FontId::proportional(14.0))
                                .strong()
                        );
                        
                        // Apply custom glowing purple fill to the main action button
                        if !is_decompiling && has_inputs {
                            btn = btn.fill(egui::Color32::from_rgb(124, 58, 237));
                        }
                        
                        if ui.add(btn).clicked() {
                            self.start_decompilation();
                        }
                    });
                    
                    // Simple reset/clear logs button
                    if ui.button("Clear Console").clicked() {
                        self.log_messages.clear();
                        if self.status != DecompileStatus::Decompiling {
                            self.status = DecompileStatus::Idle;
                        }
                    }
                    
                    // Visual Status Pill
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match &self.status {
                            DecompileStatus::Idle => {
                                ui.label(egui::RichText::new("Idle").color(egui::Color32::from_rgb(156, 163, 175)));
                            }
                            DecompileStatus::Decompiling => {
                                ui.label(egui::RichText::new("Decompiling...").strong().color(egui::Color32::from_rgb(251, 191, 36)));
                            }
                            DecompileStatus::Success => {
                                ui.label(egui::RichText::new("Success!").strong().color(egui::Color32::from_rgb(52, 211, 153)));
                            }
                            DecompileStatus::Error(_) => {
                                ui.label(egui::RichText::new("Failed").strong().color(egui::Color32::from_rgb(248, 113, 113)));
                            }
                        }
                    });
                });
                
                ui.add_space(8.0);
                
                // Logging Area / Console (Sleek terminal emulator styling)
                ui.label(egui::RichText::new("Console Logs:").strong().color(egui::Color32::from_rgb(156, 163, 175)));
                
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(8, 8, 10))
                    .rounding(6.0)
                    .inner_margin(8.0)
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(28, 28, 35)))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(250.0)
                            .min_scrolled_height(250.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                if self.log_messages.is_empty() {
                                    ui.label(
                                        egui::RichText::new("Console is empty. Select inputs and click Decompile to start.")
                                            .font(egui::FontId::monospace(12.0))
                                            .color(egui::Color32::from_rgb(80, 80, 95))
                                    );
                                } else {
                                    for line in &self.log_messages {
                                        let text_color = if line.starts_with("[ERROR]") {
                                            egui::Color32::from_rgb(248, 113, 113) // Soft Red
                                        } else if line.starts_with("[SYSTEM]") {
                                            egui::Color32::from_rgb(34, 211, 238)  // Soft Cyan
                                        } else {
                                            egui::Color32::from_rgb(228, 228, 231) // Warm Off-white
                                        };
                                        
                                        ui.label(
                                            egui::RichText::new(line)
                                                .font(egui::FontId::monospace(12.0))
                                                .color(text_color)
                                        );
                                    }
                                }
                            });
                    });
            });
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CFR Decompiler Wrapper")
            .with_inner_size([650.0, 480.0])
            .with_resizable(true),
        ..Default::default()
    };
    
    eframe::run_native(
        "CFR Decompiler GUI",
        options,
        Box::new(|cc| Box::new(DecompilerApp::new(cc))),
    )
}
