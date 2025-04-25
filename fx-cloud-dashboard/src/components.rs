use leptos::prelude::*;

pub enum ButtonVariant {
    Outline,
    Ghost,
}

#[component]
pub fn button(
    #[prop(optional)]
    variant: Option<ButtonVariant>,
    class: impl Into<String>,
    children: Children,
) -> impl IntoView {
    let btn_class= format!(
        "{} {}",
        match variant {
            None => "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 h-10 px-4 py-2",
            Some(ButtonVariant::Outline) => "inline-flex items-center justify-center gap-2 whitespace-nowrap text-sm font-medium ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 border h-9 rounded-md px-3",
            Some(ButtonVariant::Ghost) => "inline-flex items-center gap-2 whitespace-nowrap rounded-md text-sm font-medium ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0 h-10 px-4 py-2 justify-start",
        },
        class.into()
    );

    view! {
        <button class=btn_class>
            { children() }
        </button>
    }
}

pub enum BadgeVariant {
    Default,
    Destructive,
}

#[component]
pub fn badge(
    variant: BadgeVariant,
    class: impl Into<String>,
    children: Children,
) -> impl IntoView {
    let badge_class = format!(
        "{} {}",
        match variant {
            BadgeVariant::Default => "inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 border-transparent",
            BadgeVariant::Destructive => "inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 border-transparent",
        },
        class.into(),
    );

    view! { <div class=badge_class>{ children() }</div> }
}
