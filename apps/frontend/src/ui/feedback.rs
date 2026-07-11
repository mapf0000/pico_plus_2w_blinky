use super::super::*;

#[derive(Properties, PartialEq, Clone)]
pub(crate) struct ToastProps {
    pub toast: Option<(String, bool)>,
}
#[function_component(ToastBar)]
pub(crate) fn toast_bar(props: &ToastProps) -> Html {
    if let Some((msg, ok)) = &props.toast {
        let class = if *ok { "toast ok" } else { "toast err" };
        let role = if *ok { "status" } else { "alert" };
        html! {
          <div role={role} aria-live="polite" class={class.to_string()}>
            { msg }
          </div>
        }
    } else {
        html! {}
    }
}
