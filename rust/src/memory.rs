//! Speicher an das Betriebssystem zurückgeben.
//!
//! glibc legt je Thread eine eigene Speicherarena an (bis zu 8 × Kerne) und gibt freigegebenen
//! Speicher mitten im Heap nicht von selbst zurück. Für einen Client mit einer Handvoll Threads,
//! der einmal beim Laden der Texturen viel Zwischenspeicher braucht, heißt das: Der Prozess
//! behält die Spitze dauerhaft. Beides lässt sich mit zwei glibc-Aufrufen abstellen; auf allen
//! anderen Plattformen (Windows, musl, macOS) sind die Funktionen leer.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
        fn malloc_trim(pad: usize) -> i32;
    }
    /// `M_ARENA_MAX` aus `<malloc.h>`.
    const M_ARENA_MAX: i32 = -8;

    pub fn limit_arenas() {
        // SAFETY: reine Einstellungsfunktion der C-Bibliothek, vor dem Start weiterer Threads.
        unsafe {
            mallopt(M_ARENA_MAX, 1);
        }
    }

    pub fn trim() {
        // SAFETY: gibt nur bereits freigegebene Seiten zurück; jederzeit aufrufbar.
        unsafe {
            malloc_trim(0);
        }
    }
}

/// Einmal ganz am Anfang von `main`, bevor ein zweiter Thread läuft: alle Threads teilen sich
/// eine Arena. Der Client verteilt seine Arbeit auf wenige, meist schlafende Threads – mehr
/// Arenen bringen hier keine Geschwindigkeit, nur Verschnitt.
pub fn limit_arenas() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    glibc::limit_arenas();
}

/// Nach einer großen, vorübergehenden Belegung (Texturen laden, Welt verlassen) aufrufen.
pub fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    glibc::trim();
}
