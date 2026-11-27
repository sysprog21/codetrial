pub mod csv {
    pub struct Line(pub String);

    pub fn header() -> Line {
        Line(super::rows::SEPARATOR.to_string())
    }
}

pub mod rows {
    pub const SEPARATOR: &str = ",";

    pub fn width(line: &super::csv::Line) -> usize {
        line.0.split(SEPARATOR).count()
    }
}
