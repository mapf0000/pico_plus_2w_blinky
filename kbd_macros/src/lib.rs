use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{quote, ToTokens};
use syn::{parse::Parse, parse_macro_input, Ident, LitInt, LitStr, Token};

// Parse input: "text", optional , delay
struct TextInput {
    s: LitStr,
    delay: Option<LitInt>,
}

impl Parse for TextInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let s: LitStr = input.parse()?;
        let delay = if input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
            let d: LitInt = input.parse()?;
            Some(d)
        } else {
            None
        };
        Ok(TextInput { s, delay })
    }
}

fn map_char(c: char) -> Option<(&'static str, bool)> {
    // returns (KEY_CONST_IDENT, needs_shift)
    Some(match c {
        // Whitespace and control
        '\n' | '\r' => ("KEY_ENTER", false),
        '\t' => ("KEY_TAB", false),
        ' ' => ("KEY_SPACE", false),

        // Letters
        'a'..='z' => {
            let idx = (c as u8) - b'a';
            let keys = [
                "KEY_A", "KEY_B", "KEY_C", "KEY_D", "KEY_E", "KEY_F", "KEY_G", "KEY_H",
                "KEY_I", "KEY_J", "KEY_K", "KEY_L", "KEY_M", "KEY_N", "KEY_O", "KEY_P",
                "KEY_Q", "KEY_R", "KEY_S", "KEY_T", "KEY_U", "KEY_V", "KEY_W", "KEY_X",
                "KEY_Y", "KEY_Z",
            ];
            (keys[idx as usize], false)
        }
        'A'..='Z' => {
            let idx = (c as u8) - b'A';
            let keys = [
                "KEY_A", "KEY_B", "KEY_C", "KEY_D", "KEY_E", "KEY_F", "KEY_G", "KEY_H",
                "KEY_I", "KEY_J", "KEY_K", "KEY_L", "KEY_M", "KEY_N", "KEY_O", "KEY_P",
                "KEY_Q", "KEY_R", "KEY_S", "KEY_T", "KEY_U", "KEY_V", "KEY_W", "KEY_X",
                "KEY_Y", "KEY_Z",
            ];
            (keys[idx as usize], true)
        }

        // Number row and shifted symbols
        '1' => ("KEY_1", false),
        '2' => ("KEY_2", false),
        '3' => ("KEY_3", false),
        '4' => ("KEY_4", false),
        '5' => ("KEY_5", false),
        '6' => ("KEY_6", false),
        '7' => ("KEY_7", false),
        '8' => ("KEY_8", false),
        '9' => ("KEY_9", false),
        '0' => ("KEY_0", false),
        '!' => ("KEY_1", true),
        '@' => ("KEY_2", true),
        '#' => ("KEY_3", true),
        '$' => ("KEY_4", true),
        '%' => ("KEY_5", true),
        '^' => ("KEY_6", true),
        '&' => ("KEY_7", true),
        '*' => ("KEY_8", true),
        '(' => ("KEY_9", true),
        ')' => ("KEY_0", true),

        // Punctuation cluster
        '-' => ("KEY_MINUS", false),
        '_' => ("KEY_MINUS", true),
        '=' => ("KEY_EQUAL", false),
        '+' => ("KEY_EQUAL", true),
        '[' => ("KEY_LEFT_BRACKET", false),
        '{' => ("KEY_LEFT_BRACKET", true),
        ']' => ("KEY_RIGHT_BRACKET", false),
        '}' => ("KEY_RIGHT_BRACKET", true),
        '\\' => ("KEY_BACKSLASH", false),
        '|' => ("KEY_BACKSLASH", true),
        ';' => ("KEY_SEMICOLON", false),
        ':' => ("KEY_SEMICOLON", true),
        '\'' => ("KEY_APOSTROPHE", false),
        '"' => ("KEY_APOSTROPHE", true),
        '`' => ("KEY_GRAVE", false),
        '~' => ("KEY_GRAVE", true),
        ',' => ("KEY_COMMA", false),
        '<' => ("KEY_COMMA", true),
        '.' => ("KEY_DOT", false),
        '>' => ("KEY_DOT", true),
        '/' => ("KEY_SLASH", false),
        '?' => ("KEY_SLASH", true),

        _ => return None,
    })
}

fn build_items(s: &str, delay_tokens: Option<proc_macro2::TokenStream>) -> proc_macro2::TokenStream {
    let mut parts: Vec<proc_macro2::TokenStream> = Vec::new();
    for ch in s.chars() {
        let (key_name, needs_shift) = match map_char(ch) {
            Some(m) => m,
            None => {
                let msg = format!("Unsupported character in text_events!: {:?}", ch);
                return quote! { compile_error!(#msg); };
            }
        };
        let key_ident = Ident::new(key_name, Span::call_site());

        let mods = if needs_shift {
            quote! { crate::keyboard::MOD_LSHIFT }
        } else {
            quote! { 0 }
        };

        parts.push(quote! { crate::keyboard::KeyScriptEvent::Tap { key: crate::keyboard::#key_ident, mods: #mods } });

        if let Some(d) = &delay_tokens {
            parts.push(quote! { crate::keyboard::KeyScriptEvent::DelayMs((#d) as u32) });
        }
    }

    quote! { #(#parts),* }
}

/// Expand to a comma-separated list of `KeyScriptEvent` items for use inside an array.
#[proc_macro]
pub fn text_items(input: TokenStream) -> TokenStream {
    let TextInput { s, delay } = parse_macro_input!(input as TextInput);
    let ds = delay.map(|d| d.to_token_stream());
    let stream = build_items(&s.value(), ds);
    TokenStream::from(stream)
}

/// Expand to an expression: `&[KeyScriptEvent; N]` with optional per-char delay in ms.
#[proc_macro]
pub fn text_events(input: TokenStream) -> TokenStream {
    let TextInput { s, delay } = parse_macro_input!(input as TextInput);
    let ds = delay.map(|d| d.to_token_stream());
    let items = build_items(&s.value(), ds);
    let stream = quote! { &[ #items ] };
    TokenStream::from(stream)
}
