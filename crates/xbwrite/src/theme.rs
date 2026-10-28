//! Visual theme: palettes, fonts and a few custom widgets.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use eframe::egui::{
    self, epaint::Shadow, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Margin,
    Response, RichText, Sense, Stroke, TextStyle, Theme, Ui, Vec2, Visuals, WidgetText,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeChoice {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
            Self::System => egui::ThemePreference::System,
        }
    }
}

/// Colour scheme family; each has a dark and a light variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemeStyle {
    /// Console green on charcoal / white, pill buttons.
    #[default]
    Duke,
    /// A cooler violet look with squarer corners.
    Violet,
}

static STYLE: AtomicU8 = AtomicU8::new(0);

pub fn style() -> ThemeStyle {
    match STYLE.load(Ordering::Relaxed) {
        1 => ThemeStyle::Violet,
        _ => ThemeStyle::Duke,
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    pub input: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
    pub info: Color32,
    pub alt: Color32,
    pub shadow: Color32,
    /// Corner radius of the big action buttons (34 px tall).
    pub button_radius: u8,
    /// Corner radius of regular widgets (28 px tall).
    pub widget_radius: u8,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const VIOLET_DARK: Palette = Palette {
    bg: hex(0x0E1015),
    panel: hex(0x14171D),
    card: hex(0x191C24),
    control: hex(0x232833),
    control_hover: hex(0x2C323E),
    control_active: hex(0x363D4B),
    input: hex(0x0B0D11),
    border: hex(0x252A34),
    border_strong: hex(0x3A4250),
    text: hex(0xE8EAF0),
    text_dim: hex(0x9AA3B2),
    text_faint: hex(0x5F6776),
    accent: hex(0x7C6CFF),
    accent_hover: hex(0x9184FF),
    on_accent: Color32::WHITE,
    ok: hex(0x3DD68C),
    warn: hex(0xF5B84B),
    err: hex(0xFF6B6B),
    info: hex(0x5AA9FF),
    alt: hex(0xE58AF5),
    shadow: Color32::from_black_alpha(90),
    button_radius: 9,
    widget_radius: 8,
};

pub const VIOLET_LIGHT: Palette = Palette {
    bg: hex(0xF3F4F7),
    panel: hex(0xFFFFFF),
    card: hex(0xFFFFFF),
    control: hex(0xEEF0F4),
    control_hover: hex(0xE3E6EC),
    control_active: hex(0xD7DBE3),
    input: hex(0xF7F8FA),
    border: hex(0xE2E5EB),
    border_strong: hex(0xC5CBD5),
    text: hex(0x171A21),
    text_dim: hex(0x5F6775),
    text_faint: hex(0x9AA0AB),
    accent: hex(0x5B4BEA),
    accent_hover: hex(0x6E60F0),
    on_accent: Color32::WHITE,
    ok: hex(0x12A66A),
    warn: hex(0xC07A06),
    err: hex(0xDC3F48),
    info: hex(0x2F7BEA),
    alt: hex(0xB042C4),
    shadow: Color32::from_black_alpha(22),
    button_radius: 9,
    widget_radius: 8,
};

/// The "Duke" style: matte charcoal with the console's green.
pub const DUKE_DARK: Palette = Palette {
    bg: hex(0x141514),
    panel: hex(0x1B1C1B),
    card: hex(0x232523),
    control: hex(0x2F322F),
    control_hover: hex(0x3A3D3A),
    control_active: hex(0x464A46),
    input: hex(0x101110),
    border: hex(0x2D302D),
    border_strong: hex(0x495049),
    text: hex(0xF3F5F3),
    text_dim: hex(0xB0B6B0),
    text_faint: hex(0x6E756E),
    accent: hex(0x1E9E28),
    accent_hover: hex(0x27B832),
    on_accent: Color32::WHITE,
    ok: hex(0x63D86B),
    warn: hex(0xFFC145),
    err: hex(0xFF5C64),
    info: hex(0x2FC6EA),
    alt: hex(0xC9A0FF),
    shadow: Color32::from_black_alpha(110),
    button_radius: 17,
    widget_radius: 14,
};

pub const DUKE_LIGHT: Palette = Palette {
    bg: hex(0xEFF1EF),
    panel: hex(0xFFFFFF),
    card: hex(0xFFFFFF),
    control: hex(0xEBEEEB),
    control_hover: hex(0xDFE3DF),
    control_active: hex(0xD2D7D2),
    input: hex(0xF7F8F7),
    border: hex(0xE0E3E0),
    border_strong: hex(0xC4CAC4),
    text: hex(0x222522),
    text_dim: hex(0x575E57),
    text_faint: hex(0x9AA19A),
    accent: hex(0x107C10),
    accent_hover: hex(0x0B5E0B),
    on_accent: Color32::WHITE,
    ok: hex(0x1E9E3A),
    warn: hex(0xB87503),
    err: hex(0xC8102E),
    info: hex(0x0A97C2),
    alt: hex(0x7B4FC8),
    shadow: Color32::from_black_alpha(20),
    button_radius: 17,
    widget_radius: 14,
};

pub fn palette_for(style: ThemeStyle, dark: bool) -> Palette {
    match (style, dark) {
        (ThemeStyle::Duke, true) => DUKE_DARK,
        (ThemeStyle::Duke, false) => DUKE_LIGHT,
        (ThemeStyle::Violet, true) => VIOLET_DARK,
        (ThemeStyle::Violet, false) => VIOLET_LIGHT,
    }
}

pub fn palette(ui: &Ui) -> Palette {
    palette_for(style(), ui.visuals().dark_mode)
}

/// The palette for the context's current theme, for drawing outside a `Ui`.
pub fn palette_ctx(ctx: &egui::Context) -> Palette {
    palette_for(style(), ctx.theme() == Theme::Dark)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn install(ctx: &egui::Context, style: ThemeStyle, choice: ThemeChoice) {
    install_fonts(ctx);
    apply_style(ctx, style);
    ctx.set_theme(choice.preference());
}

/// Switches the colour scheme family (both dark and light variants).
pub fn apply_style(ctx: &egui::Context, style: ThemeStyle) {
    STYLE.store(style as u8, Ordering::Relaxed);
    for theme in [Theme::Dark, Theme::Light] {
        let p = palette_for(style, theme == Theme::Dark);
        ctx.set_visuals_of(theme, visuals(&p, theme == Theme::Dark));
        ctx.style_mut_of(theme, style_tweaks);
    }
}

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.input;
    v.faint_bg_color = p.control;
    v.code_bg_color = p.control;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = Shadow { offset: [0, 10], blur: 32, spread: 0, color: p.shadow };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: p.shadow };
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.err;
    v.weak_text_color = Some(p.text_dim);
    v.interact_cursor = Some(CursorIcon::PointingHand);
    v.striped = true;

    let r = CornerRadius::same(p.widget_radius);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.control;
    w.inactive.weak_bg_fill = p.control;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = p.control_hover;
    w.hovered.weak_bg_fill = p.control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.control_active;
    w.active.weak_bg_fill = p.control_active;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open = w.hovered;
    v
}

fn style_tweaks(s: &mut egui::Style) {
    s.interaction.selectable_labels = false;
    s.interaction.tooltip_delay = 0.25;
    s.animation_time = 0.14;
    s.spacing.item_spacing = Vec2::new(8.0, 8.0);
    s.spacing.button_padding = Vec2::new(12.0, 6.0);
    s.spacing.interact_size.y = 28.0;
    s.spacing.icon_width = 18.0;
    s.spacing.icon_width_inner = 10.0;
    s.spacing.window_margin = Margin::same(20);
    s.spacing.scroll = egui::style::ScrollStyle::floating();
    s.text_styles = [
        (TextStyle::Heading, semibold(22.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into();
}

/// Uses the platform UI font when available (Segoe UI on Windows, SF on macOS,
/// Inter/Noto/DejaVu on Linux), falling back to egui's bundled fonts.
fn install_fonts(ctx: &egui::Context) {
    #[cfg(target_os = "windows")]
    let (regular, bold, extra): (&[&str], &[&str], &[&str]) = (
        &["C:\\Windows\\Fonts\\segoeui.ttf"],
        &["C:\\Windows\\Fonts\\seguisb.ttf", "C:\\Windows\\Fonts\\segoeuib.ttf"],
        &["C:\\Windows\\Fonts\\seguisym.ttf"],
    );
    #[cfg(target_os = "macos")]
    let (regular, bold, extra): (&[&str], &[&str], &[&str]) =
        (&["/System/Library/Fonts/SFNS.ttf", "/Library/Fonts/Arial Unicode.ttf"], &[], &[]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let (regular, bold, extra): (&[&str], &[&str], &[&str]) = (
        &[
            "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
            "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ],
        &[
            "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
            "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf",
            "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        ],
        &[],
    );

    // CJK fallbacks, one list per script. Han characters are shared between the scripts, so the
    // first font that loads (Japanese) wins for them; Korean and Chinese add what it lacks.
    #[cfg(target_os = "windows")]
    let cjk: &[&[&str]] = &[
        &["C:\\Windows\\Fonts\\YuGothR.ttc", "C:\\Windows\\Fonts\\meiryo.ttc", "C:\\Windows\\Fonts\\msgothic.ttc"],
        &["C:\\Windows\\Fonts\\malgun.ttf"],
        &["C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\simsun.ttc"],
        &["C:\\Windows\\Fonts\\msjh.ttc"],
    ];
    #[cfg(target_os = "macos")]
    let cjk: &[&[&str]] = &[
        &["/System/Library/Fonts/\u{30d2}\u{30e9}\u{30ae}\u{30ce}\u{89d2}\u{30b4}\u{30b7}\u{30c3}\u{30af} W3.ttc"],
        &["/System/Library/Fonts/AppleSDGothicNeo.ttc"],
        &["/System/Library/Fonts/PingFang.ttc"],
    ];
    #[cfg(all(unix, not(target_os = "macos")))]
    let cjk: &[&[&str]] = &[
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/OTF/NotoSansCJK-Regular.ttc",
        ],
        &[
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ],
    ];

    let mut defs = FontDefinitions::default();
    let base: Vec<String> = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    // Registers the first of `paths` that loads as `name` and adds the name to `family`.
    let mut add = |name: String, paths: &[&str], family: &mut Vec<String>| {
        if let Some(bytes) = paths.iter().find_map(|p| std::fs::read(p).ok()) {
            defs.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
            family.push(name);
        }
    };

    let mut prop = Vec::new();
    add("ui-regular".into(), regular, &mut prop);
    let mut strong = Vec::new();
    add("ui-semibold".into(), bold, &mut strong);
    let mut fallback = base;
    add("ui-symbols".into(), extra, &mut fallback);
    for (i, paths) in cjk.iter().enumerate() {
        add(format!("ui-cjk-{i}"), paths, &mut fallback);
    }

    let mut proportional = prop.clone();
    proportional.extend(fallback.iter().cloned());
    let mut semibold_family = if strong.is_empty() { prop } else { strong };
    semibold_family.extend(fallback.iter().cloned());

    defs.families.insert(FontFamily::Proportional, proportional);
    defs.families.insert(FontFamily::Name("semibold".into()), semibold_family);
    if let Some(mono) = defs.families.get_mut(&FontFamily::Monospace) {
        mono.extend(fallback.iter().filter(|n| n.starts_with("ui-")).cloned());
    }
    ctx.set_fonts(defs);
}

// ------------------------------------------------------------------ widgets

/// Filled accent button for the main action.
pub fn primary_button(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    primary_button_sized(ui, enabled, text, Vec2::new(0.0, 34.0))
}

/// [`primary_button`] with a minimum size.
pub fn primary_button_sized(ui: &mut Ui, enabled: bool, text: &str, min_size: Vec2) -> Response {
    let p = palette(ui);
    let id = ui.next_auto_id();
    let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered()) && enabled;
    let fill = if hovered { p.accent_hover } else { p.accent };
    let btn = egui::Button::new(RichText::new(text).font(semibold(14.0)).color(p.on_accent))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(p.button_radius)
        .min_size(min_size);
    ui.add_enabled(enabled, btn)
}

/// Neutral filled button.
pub fn secondary_button(ui: &mut Ui, enabled: bool, text: impl Into<WidgetText>) -> Response {
    let r = palette(ui).button_radius;
    ui.add_enabled(enabled, egui::Button::new(text).corner_radius(r).min_size(Vec2::new(0.0, 34.0)))
}

/// Borderless button that only shows a background on hover.
pub fn ghost_button(ui: &mut Ui, enabled: bool, text: impl Into<WidgetText>) -> Response {
    let r = palette(ui).widget_radius;
    ui.add_enabled(enabled, egui::Button::new(text).frame_when_inactive(false).corner_radius(r))
}

/// Small square icon button.
pub fn icon_button(ui: &mut Ui, enabled: bool, icon: &str, tooltip: &str) -> Response {
    let p = palette(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(icon).size(16.0).color(p.text_dim))
            .frame_when_inactive(false)
            .corner_radius(p.widget_radius.max(8) + 2)
            .min_size(Vec2::splat(32.0)),
    )
    .on_hover_text(tooltip)
}

/// Rounded label with a soft tinted background.
pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    let dark = ui.visuals().dark_mode;
    egui::Frame::new()
        .fill(color.gamma_multiply(if dark { 0.16 } else { 0.12 }))
        .corner_radius(20)
        .inner_margin(Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text.into()).font(semibold(12.0)).color(color));
        })
        .response
}

/// iOS-style switch with a label and optional description.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, hint: &str) -> Response {
    let p = palette(ui);
    let r = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let size = Vec2::new(34.0, 20.0);
            let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
            let label_resp = ui.add(egui::Label::new(label).sense(Sense::click()));
            if resp.clicked() || label_resp.clicked() {
                *on = !*on;
                resp.mark_changed();
            }
            let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
            let off_fill = if resp.hovered() || label_resp.hovered() { p.control_active } else { p.control_hover };
            let fill = lerp_color(off_fill, p.accent, t);
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(10), fill);
            let cx = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
            painter.circle_filled(egui::pos2(cx, rect.center().y), 7.0, Color32::WHITE);
            resp.on_hover_cursor(CursorIcon::PointingHand) | label_resp.on_hover_cursor(CursorIcon::PointingHand)
        })
        .inner;
    if hint.is_empty() {
        r
    } else {
        r.on_hover_text(hint)
    }
}

/// Horizontal segmented control; returns true if the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let p = palette(ui);
    let mut changed = false;
    egui::Frame::new().fill(p.control).corner_radius(p.button_radius.max(10)).inner_margin(Margin::same(3)).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (opt, label) in options {
                    let selected = *value == *opt;
                    let text = RichText::new(*label).color(if selected { p.on_accent } else { p.text_dim });
                    let btn = egui::Button::new(text)
                        .fill(if selected { p.accent } else { Color32::TRANSPARENT })
                        .frame_when_inactive(selected)
                        .corner_radius(p.widget_radius)
                        .min_size(Vec2::new(72.0, 26.0));
                    if ui.add(btn).clicked() && !selected {
                        *value = *opt;
                        changed = true;
                    }
                }
            });
        },
    );
    changed
}

/// Thin rounded progress bar with smooth animation.
pub fn progress_bar(ui: &mut Ui, id: egui::Id, fraction: f32, width: f32) -> Response {
    let p = palette(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 6.0), Sense::hover());
    let f = ui.ctx().animate_value_with_time(id, fraction.clamp(0.0, 1.0), 0.25);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), p.control_hover);
    if f > 0.0 {
        let mut fill = rect;
        fill.set_width((rect.width() * f).max(6.0));
        painter.rect_filled(fill, CornerRadius::same(3), p.accent);
    }
    resp
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Draws a dashed rounded-ish rectangle outline.
pub fn dashed_rect(painter: &egui::Painter, rect: egui::Rect, stroke: Stroke) {
    let pts = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    painter.extend(egui::Shape::dashed_line(&pts, stroke, 8.0, 6.0));
}
