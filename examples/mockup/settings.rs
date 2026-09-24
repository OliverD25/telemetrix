pub const THEMES: [&str; 2] = ["minimalist", "matrix"];
pub const MATRIX: usize = 1;
const FPS: [u32; 6] = [5, 10, 15, 20, 30, 60];
const COLORS: [&str; 4] = ["green", "amber", "cyan", "white"];
pub const PLUGINS: [&str; 6] = [
    "weather",
    "crypto",
    "network_ping",
    "clock",
    "uptime",
    "broken_plugin",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Theme,
    Fps,
    TempUnit,
    Bytes,
    CpuWarn,
    DiskWarn,
    Density,
    Speed,
    Color,
    Sparklines,
    Plugin(usize),
}

pub enum Row {
    Section(&'static str),
    Edit(Field),
    Fixed {
        key: &'static str,
        value: &'static str,
    },
}

/// Every setting is an index into its list of allowed values, so Enter can
/// cycle any row the same way.
pub struct Settings {
    pub theme: usize,
    fps: usize,
    pub fahrenheit: bool,
    pub decimal: bool,
    cpu_warn: usize,
    disk_warn: usize,
    density: usize,
    speed: usize,
    pub color: usize,
    pub sparklines: bool,
    pub plugins: [bool; PLUGINS.len()],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: MATRIX,
            fps: 2,
            fahrenheit: false,
            decimal: false,
            cpu_warn: 6,
            disk_warn: 4,
            density: 4,
            speed: 1,
            color: 0,
            sparklines: true,
            plugins: [true; PLUGINS.len()],
        }
    }
}

pub fn cycle(i: usize, len: usize, dir: i32) -> usize {
    let len = len as i64;
    (i as i64 + i64::from(dir)).rem_euclid(len) as usize
}

impl Settings {
    pub fn fps(&self) -> u32 {
        FPS[self.fps]
    }

    pub fn cpu_warn(&self) -> f32 {
        50.0 + 5.0 * self.cpu_warn as f32
    }

    pub fn disk_warn(&self) -> f32 {
        70.0 + 5.0 * self.disk_warn as f32
    }

    pub fn density(&self) -> f32 {
        0.1 * (self.density + 1) as f32
    }

    pub fn speed(&self) -> f32 {
        0.5 * (self.speed + 1) as f32
    }

    pub fn theme_name(&self) -> &'static str {
        THEMES[self.theme]
    }

    pub fn step(&mut self, field: Field, dir: i32) {
        let next = cycle(self.index(field), option_count(field), dir);
        self.set_index(field, next);
    }

    fn index(&self, field: Field) -> usize {
        match field {
            Field::Theme => self.theme,
            Field::Fps => self.fps,
            Field::TempUnit => usize::from(self.fahrenheit),
            Field::Bytes => usize::from(self.decimal),
            Field::CpuWarn => self.cpu_warn,
            Field::DiskWarn => self.disk_warn,
            Field::Density => self.density,
            Field::Speed => self.speed,
            Field::Color => self.color,
            Field::Sparklines => usize::from(self.sparklines),
            Field::Plugin(p) => usize::from(self.plugins[p]),
        }
    }

    fn set_index(&mut self, field: Field, i: usize) {
        match field {
            Field::Theme => self.theme = i,
            Field::Fps => self.fps = i,
            Field::TempUnit => self.fahrenheit = i == 1,
            Field::Bytes => self.decimal = i == 1,
            Field::CpuWarn => self.cpu_warn = i,
            Field::DiskWarn => self.disk_warn = i,
            Field::Density => self.density = i,
            Field::Speed => self.speed = i,
            Field::Color => self.color = i,
            Field::Sparklines => self.sparklines = i == 1,
            Field::Plugin(p) => self.plugins[p] = i == 1,
        }
    }

    pub fn value_text(&self, field: Field) -> String {
        let on_off = |b: bool| if b { "on" } else { "off" }.to_string();
        match field {
            Field::Theme => self.theme_name().to_string(),
            Field::Fps => self.fps().to_string(),
            Field::TempUnit => if self.fahrenheit {
                "fahrenheit"
            } else {
                "celsius"
            }
            .to_string(),
            Field::Bytes => if self.decimal {
                "decimal (GB)"
            } else {
                "binary (GiB)"
            }
            .to_string(),
            Field::CpuWarn => format!("{} %", self.cpu_warn()),
            Field::DiskWarn => format!("{} %", self.disk_warn()),
            Field::Density => format!("{:.1}", self.density()),
            Field::Speed => format!("{:.1}", self.speed()),
            Field::Color => COLORS[self.color].to_string(),
            Field::Sparklines => on_off(self.sparklines),
            Field::Plugin(p) => on_off(self.plugins[p]),
        }
    }
}

fn option_count(field: Field) -> usize {
    match field {
        Field::Theme => THEMES.len(),
        Field::Fps => FPS.len(),
        Field::CpuWarn => 10,
        Field::DiskWarn => 6,
        Field::Density => 10,
        Field::Speed => 6,
        Field::Color => COLORS.len(),
        Field::TempUnit | Field::Bytes | Field::Sparklines | Field::Plugin(_) => 2,
    }
}

pub fn key(field: Field) -> &'static str {
    match field {
        Field::Theme => "theme",
        Field::Fps => "fps",
        Field::TempUnit => "temperature",
        Field::Bytes => "bytes",
        Field::CpuWarn => "cpu_warn_pct",
        Field::DiskWarn => "disk_warn_pct",
        Field::Density => "density",
        Field::Speed => "speed",
        Field::Color => "color",
        Field::Sparklines => "show_sparklines",
        Field::Plugin(p) => PLUGINS[p],
    }
}

pub fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row::Section("general"),
        Row::Edit(Field::Theme),
        Row::Edit(Field::Fps),
        Row::Fixed {
            key: "plugins_dir",
            value: "\"plugins\"",
        },
        Row::Fixed {
            key: "log_file",
            value: "\"\"",
        },
        Row::Section("units"),
        Row::Edit(Field::TempUnit),
        Row::Edit(Field::Bytes),
        Row::Section("thresholds"),
        Row::Edit(Field::CpuWarn),
        Row::Edit(Field::DiskWarn),
        Row::Section("theme.matrix"),
        Row::Edit(Field::Density),
        Row::Edit(Field::Speed),
        Row::Edit(Field::Color),
        Row::Section("theme.minimalist"),
        Row::Edit(Field::Sparklines),
        Row::Section("plugins (enabled)"),
    ];
    rows.extend((0..PLUGINS.len()).map(|p| Row::Edit(Field::Plugin(p))));
    rows
}

/// Indices of the rows the cursor can land on (everything but section headers).
pub fn selectable(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| !matches!(r, Row::Section(_)))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_wraps_both_ways() {
        assert_eq!(cycle(0, 3, -1), 2);
        assert_eq!(cycle(2, 3, 1), 0);
        assert_eq!(cycle(1, 3, 1), 2);
    }

    #[test]
    fn defaults_match_the_plan() {
        let s = Settings::default();
        assert_eq!(s.theme_name(), "matrix");
        assert_eq!(s.fps(), 15);
        assert_eq!(s.cpu_warn(), 80.0);
        assert_eq!(s.disk_warn(), 90.0);
        assert_eq!(s.value_text(Field::Density), "0.5");
        assert_eq!(s.value_text(Field::Speed), "1.0");
    }

    #[test]
    fn step_moves_through_every_range() {
        let mut s = Settings::default();
        s.step(Field::CpuWarn, 1);
        s.step(Field::CpuWarn, 1);
        s.step(Field::CpuWarn, 1);
        assert_eq!(s.cpu_warn(), 95.0);
        s.step(Field::CpuWarn, 1);
        assert_eq!(s.cpu_warn(), 50.0);
        s.step(Field::Fps, -1);
        assert_eq!(s.fps(), 10);
        s.step(Field::Speed, -1);
        s.step(Field::Speed, -1);
        assert_eq!(s.value_text(Field::Speed), "3.0");
        s.step(Field::Plugin(5), 1);
        assert!(!s.plugins[5]);
        s.step(Field::TempUnit, -1);
        assert!(s.fahrenheit);
    }

    #[test]
    fn cursor_skips_sections() {
        let rows = rows();
        let sel = selectable(&rows);
        assert!(sel.iter().all(|&i| !matches!(rows[i], Row::Section(_))));
        assert_eq!(sel.len(), 10 + 2 + PLUGINS.len());
    }
}
