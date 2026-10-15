//! The desktop's error catalogue (UX-044): every error the window can show for a failed file or
//! start-up action has a code, what failed, why it matters and what to do, written once here.
//!
//! The text on screen is the catalogue's plain-language text plus a short cause. The technical
//! message the failing code returned (an I/O error, a file-format error) is kept apart and is
//! what the "Copy details" action puts on the clipboard. Raw panic text, backtraces and bare exit
//! codes never go on screen: [`looks_raw`] recognises them and the cause is replaced.

/// One catalogued error.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// A stable code to quote in a report: `FARIS-APP-` and a number.
    pub code: &'static str,
    /// What failed, as a sentence.
    pub what: &'static str,
    /// Why it matters, or what the likely cause is.
    pub why: &'static str,
    /// What to do next.
    pub fix: &'static str,
}

pub const STUDY_OPEN: Entry = Entry {
    code: "FARIS-APP-001",
    what: "The study file could not be opened.",
    why: "The file is missing, is not a FARIS study, was changed after it was saved, or is damaged; nothing in the window was changed.",
    fix: "Check that the path is right and the file is the one FARIS saved, then open it again; if it still fails, copy the details and keep the file for the report.",
};

pub const STUDY_SAVE: Entry = Entry {
    code: "FARIS-APP-002",
    what: "The study file could not be saved.",
    why: "The folder may be read-only or full, or another program is using the file; the study in the window is unchanged and is not lost.",
    fix: "Choose another folder with Save as, or free space and try Save again.",
};

pub const GRAPHICS_START: Entry = Entry {
    code: "FARIS-APP-003",
    what: "FARIS could not start its 3D viewport.",
    why: "The viewport needs a Vulkan, Metal or DirectX 12 graphics driver and none could be used.",
    fix: "Update the graphics driver, or on Linux install the Vulkan driver for your card (for example mesa-vulkan-drivers) and check that `vulkaninfo` lists a device, then start FARIS again. The command line (`faris`) works without a display.",
};

/// Every catalogued error, for the lint.
#[cfg(test)]
pub const CATALOGUE: [&Entry; 3] = [&STUDY_OPEN, &STUDY_SAVE, &GRAPHICS_START];

/// Marks of text a person should not have to read: panic messages, backtraces, bare exit codes.
const RAW_MARKS: [&str; 7] = [
    "panicked at",
    "stack backtrace",
    "RUST_BACKTRACE",
    "exit code",
    "exit status",
    "SIGSEGV",
    "unwrap()",
];

/// True for text that looks like a panic, a backtrace or a bare exit code.
pub fn looks_raw(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    RAW_MARKS
        .iter()
        .any(|mark| lower.contains(&mark.to_ascii_lowercase()))
}

/// A catalogued error with the cause that was found and the technical message behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserError {
    pub entry: &'static Entry,
    /// What the failing action named (a file name, say), for the sentence on screen.
    pub subject: String,
    /// The technical message, exactly as returned; copied by "Copy details".
    pub details: String,
}

impl UserError {
    pub fn new(
        entry: &'static Entry,
        subject: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            entry,
            subject: subject.into(),
            details: details.into(),
        }
    }

    /// The cause as a short line for the screen: the technical message's first line, unless it
    /// looks raw.
    fn cause(&self) -> String {
        let first = self.details.lines().next().unwrap_or("").trim();
        if first.is_empty() || looks_raw(first) || looks_raw(&self.details) {
            "no readable cause was returned; the technical message is in Copy details".to_owned()
        } else {
            first.to_owned()
        }
    }

    /// What the window shows: code, what failed, the cause, why and the next step.
    pub fn text(&self) -> String {
        let subject = if self.subject.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.subject)
        };
        format!(
            "{}{subject} {} Cause: {}. {} Next step: {}",
            self.entry.what,
            self.entry.code,
            self.cause().trim_end_matches('.'),
            self.entry.why,
            self.entry.fix
        )
    }

    /// What "Copy details" puts on the clipboard: the code, the subject and the technical message.
    pub fn copy_text(&self) -> String {
        format!(
            "{} {}\nsubject: {}\n{}",
            self.entry.code, self.entry.what, self.subject, self.details
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: UX-044
    #[test]
    fn every_catalogued_error_has_a_code_a_cause_and_a_fix() {
        let mut codes = std::collections::BTreeSet::new();
        for entry in CATALOGUE {
            assert!(entry.code.starts_with("FARIS-APP-"), "{}", entry.code);
            assert!(codes.insert(entry.code), "{} is used twice", entry.code);
            for (name, text) in [("what", entry.what), ("why", entry.why), ("fix", entry.fix)] {
                assert!(text.len() > 20, "{} has no {name}", entry.code);
                assert!(text.ends_with('.'), "{} {name} is a sentence", entry.code);
                assert!(!looks_raw(text), "{} {name} reads as raw", entry.code);
            }
        }
    }

    // Verifies: UX-044
    #[test]
    fn the_screen_text_names_what_why_and_next_and_hides_raw_text() {
        let error = UserError::new(
            &STUDY_OPEN,
            "demo.faris",
            "cannot open demo.faris: No such file or directory (os error 2)",
        );
        let text = error.text();
        assert!(text.contains("could not be opened") && text.contains("demo.faris"));
        assert!(text.contains("FARIS-APP-001") && text.contains("Next step:"));
        assert!(text.contains("No such file"));
        // A panic message or backtrace never reaches the screen, only the clipboard.
        let raw = "thread 'main' panicked at src/x.rs:1:1:\nstack backtrace:\n  0: foo";
        let error = UserError::new(&STUDY_SAVE, "demo.faris", raw);
        let text = error.text();
        assert!(!looks_raw(&text), "{text}");
        assert!(text.contains("Copy details") && text.contains("Next step:"));
        assert!(error.copy_text().contains("panicked at"));
        assert!(error.copy_text().contains("FARIS-APP-002"));
        assert!(looks_raw("process exited with exit code 101"));
        assert!(!looks_raw("No such file or directory"));
    }
}
