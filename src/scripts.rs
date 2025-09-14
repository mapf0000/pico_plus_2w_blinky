/// Minimal metadata exposed to the UI.
pub struct ScriptInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Optional DSL preview so the UI can insert into textarea.
    pub dsl_preview: Option<&'static str>,
}

/// Declarative macro to define built-in scripts once and generate:
/// - `BuiltinScript` enum
/// - `SCRIPTS` metadata list
/// - `parse_id()` string->enum mapping
/// - `run_builtin()` async dispatcher
/// - A compile-time DSL string for each script (kept in sync with runtime)
///
/// The `seq` uses a structured mini-DSL with the same commands as the HTTP DSL:
///   tap(ENTER);
///   modtap(LGUI | LSHIFT, N);
///   delay(400);
///   text("Terminal", 10);
///   call(hello_world);
///
/// Keys are bare names (ENTER, Z, SLASH, N, F1, PAGE_UP, etc.).
/// Modifiers are bare names (LCTRL, LSHIFT, LALT, LGUI, RCTRL, RSHIFT, RALT, RGUI) combined with `|`.
// Helpers to stringify modifier lists (with '+')
#[doc(hidden)]
#[macro_export]
macro_rules! __mods_str {
    ( $m:ident ) => { stringify!($m) };
    ( $m:ident | $($rest:tt)+ ) => { concat!( stringify!($m), "+", $crate::__mods_str!( $($rest)+ ) ) };
}

// Convert the sequence into a single DSL string at compile time.
#[doc(hidden)]
#[macro_export]
macro_rules! __dsl_lines {
    () => { "" };
    ( tap( $key:ident ) ; $($rest:tt)* ) => {
        concat!( "tap ", stringify!($key), "\n", $crate::__dsl_lines!( $($rest)* ) )
    };
    ( modtap( $mods:tt , $key:ident ) ; $($rest:tt)* ) => {
        concat!( "modtap ", $crate::__mods_str!($mods), "+", stringify!($key), "\n", $crate::__dsl_lines!( $($rest)* ) )
    };
    ( delay( $ms:expr ) ; $($rest:tt)* ) => {
        concat!( "delay ", stringify!($ms), "\n", $crate::__dsl_lines!( $($rest)* ) )
    };
    ( text( $s:literal , $ms:expr ) ; $($rest:tt)* ) => {
        concat!( "text ", $s, " ", stringify!($ms), "\n", $crate::__dsl_lines!( $($rest)* ) )
    };
    ( text( $s:literal ) ; $($rest:tt)* ) => {
        concat!( "text ", $s, "\n", $crate::__dsl_lines!( $($rest)* ) )
    };
    ( call( $id:ident ) ; $($rest:tt)* ) => {
        concat!( "call ", stringify!($id), "\n", $crate::__dsl_lines!( $($rest)* ) )
    };
    // Allow trailing single item forms
    ( tap( $key:ident ) ; ) => { concat!( "tap ", stringify!($key), "\n" ) };
    ( modtap( $mods:tt , $key:ident ) ; ) => { concat!( "modtap ", $crate::scripts::__mods_str!($mods), "+", stringify!($key), "\n" ) };
    ( delay( $ms:expr ) ; ) => { concat!( "delay ", stringify!($ms), "\n" ) };
    ( text( $s:literal , $ms:expr ) ; ) => { concat!( "text ", $s, " ", stringify!($ms), "\n" ) };
    ( text( $s:literal ) ; ) => { concat!( "text ", $s, "\n" ) };
    ( call( $id:ident ) ; ) => { concat!( "call ", stringify!($id), "\n" ) };
}

#[macro_export]
macro_rules! define_scripts {
    (
        $(
            $Variant:ident {
                id: $id:expr,
                name: $name:expr,
                description: $desc:expr,
                seq: [ $( $seq:tt )* ]
            }
        ),+ $(,)?
    ) => {
        #[derive(Copy, Clone, Debug)]
        pub enum BuiltinScript { $( $Variant ),+ }

        pub const SCRIPTS: &[crate::scripts::ScriptInfo] = &[
            $( crate::scripts::ScriptInfo {
                id: $id,
                name: $name,
                description: $desc,
                dsl_preview: Some($crate::__dsl_lines!{ $( $seq )* }),
            } ),+
        ];

        pub fn parse_id(s: &str) -> Option<BuiltinScript> {
            match s {
                $( $id => Some(BuiltinScript::$Variant), )+
                _ => None,
            }
        }

        pub fn dsl_for_id_str(s: &str) -> Option<&'static str> {
            match s {
                $( $id => Some($crate::__dsl_lines!{ $( $seq )* }), )+
                _ => None,
            }
        }

        pub async fn run_builtin<'d, D>(
            id: BuiltinScript,
            w: &mut embassy_usb::class::hid::HidWriter<'d, D, 8>,
        ) where D: embassy_usb::driver::Driver<'d>
        {
            match id {
                $( BuiltinScript::$Variant => {
                    let _ = $crate::script_dsl::run_dsl(w, $crate::__dsl_lines!{ $( $seq )* }).await;
                } ),+
            }
        }
    };
}

// Declare all scripts in one place; generates enum, metadata and dispatcher.
crate::define_scripts! {
    OpenMacTerminal {
        id: "open_terminal",
        name: "Open macOS Terminal",
        description: "Spotlight → type 'Terminal' → Enter → New Window",
        seq: [
            modtap(LGUI, SPACE);
            delay(400);
            text("Terminal", 10);
            delay(200);
            tap(ENTER);
            delay(1500);
            modtap(LGUI, N);
        ]
    },
    MacAssistantOnce {
        id: "assistant_once",
        name: "macOS Keyboard Assistant (once)",
        description: "Guide the Keyboard Setup Assistant with ANSI hints",
        seq: [
            delay(1200);
            tap(ENTER);
            delay(800);
            tap(Z);
            delay(800);
            tap(SLASH);
            delay(800);
            tap(ENTER);
        ]
    },
    HelloWorld {
        id: "hello_world",
        name: "Hello, world (slow)",
        description: "Type 'Hello, world' with a small delay",
        seq: [ text("Hello, world", 20); ]
    },
    DemoCall {
        id: "demo_call",
        name: "Demo: Call Hello + Terminal",
        description: "Demonstrates calling other scripts by id",
        seq: [
            call(hello_world);
            delay(500);
            call(open_terminal);
        ]
    },
}
