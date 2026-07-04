#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_start_and_end() {
        let span = Span::new(3, 7);
        assert_eq!(span.start, 3);
        assert_eq!(span.end, 7);
    }

    #[test]
    fn len_is_difference() {
        let span = Span::new(3, 7);
        assert_eq!(span.len(), 4);
    }

    #[test]
    fn empty_span_is_empty() {
        let span = Span::new(5, 5);
        assert!(span.is_empty());
        assert_eq!(span.len(), 0);
    }
}
