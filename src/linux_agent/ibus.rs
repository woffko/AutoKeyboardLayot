//! The engine's side of IBus. IBus runs a D-Bus daemon of its own; the engine connects to it, offers
//! a factory and serves one engine object per input context. Every call is handled on this one
//! thread in the order it arrived, and its signals are sent before its reply, so the edits made for
//! a key reach the application before the key itself could.

use super::{
    engine::{Action, Engine, Shared},
    localization::tr,
};
use autokeyboardlayot::Language;
use std::{collections::HashMap, path::PathBuf};
use zbus::{
    blocking::{Connection, MessageIterator},
    message::{Message, Type as MessageType},
    zvariant::{ObjectPath, StructureBuilder, Value},
};

pub const COMPONENT_NAME: &str = "org.freedesktop.IBus.AutoKeyboardLayot";
pub const ENGINE_NAME: &str = "autokeyboardlayot";
const IBUS_SERVICE: &str = "org.freedesktop.IBus";
const IBUS_PATH: &str = "/org/freedesktop/IBus";
const FACTORY_PATH: &str = "/org/freedesktop/IBus/Factory";
const ENGINE_PATH_PREFIX: &str = "/org/freedesktop/IBus/Engine/";
const FACTORY_INTERFACE: &str = "org.freedesktop.IBus.Factory";
const ENGINE_INTERFACE: &str = "org.freedesktop.IBus.Engine";
const SERVICE_INTERFACE: &str = "org.freedesktop.IBus.Service";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const INTROSPECTABLE_INTERFACE: &str = "org.freedesktop.DBus.Introspectable";

const PROP_NORMAL: u32 = 0;
const PROP_TOGGLE: u32 = 1;
const PROP_RADIO: u32 = 2;
const PROP_MENU: u32 = 3;
const PROP_UNCHECKED: u32 = 0;
const PROP_CHECKED: u32 = 1;
/// GNOME Shell shows the symbol of this property in place of the input source indicator.
const PROP_INPUT_MODE: &str = "InputMode";
const PROP_AUTOMATIC: &str = "automatic";
const PROP_SETTINGS: &str = "settings";
const PROP_LAYOUT_PREFIX: &str = "layout:";

/// The address of the IBus daemon: `IBUS_ADDRESS`, or the file IBus writes for this display.
pub fn address() -> Option<String> {
    if let Some(address) = std::env::var("IBUS_ADDRESS")
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Some(address);
    }
    let text = std::fs::read_to_string(address_file()?).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("IBUS_ADDRESS="))
        .map(str::to_owned)
}

/// `~/.config/ibus/bus/<machine-id>-<host>-<display>`, as `ibus_get_socket_path` builds it.
fn address_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("IBUS_ADDRESS_FILE") {
        return Some(file.into());
    }
    let machine = std::fs::read_to_string("/etc/machine-id")
        .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
        .ok()?;
    let (host, display) = match std::env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|value| !value.is_empty())
    {
        Some(wayland) => ("unix".to_owned(), wayland),
        None => {
            let display = std::env::var("DISPLAY").ok()?;
            let (host, rest) = display.split_once(':')?;
            let number = rest.split('.').next().unwrap_or("0").to_owned();
            let host = if host.is_empty() { "unix" } else { host };
            (host.to_owned(), number)
        }
    };
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(
        config
            .join("ibus")
            .join("bus")
            .join(format!("{}-{host}-{display}", machine.trim())),
    )
}

pub fn connect(address: &str) -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::address(address)?.build()
}

fn attachments() -> HashMap<String, Value<'static>> {
    HashMap::new()
}

fn structure(builder: StructureBuilder<'static>) -> Value<'static> {
    // The layouts below are fixed, so building cannot fail.
    Value::from(builder.build().expect("a fixed IBus serialization layout"))
}

/// `IBusText`: `(sa{sv}sv)`, with an empty attribute list.
pub fn text_value(text: &str) -> Value<'static> {
    let attributes = structure(
        StructureBuilder::new()
            .add_field("IBusAttrList")
            .add_field(attachments())
            .add_field(Vec::<Value<'static>>::new()),
    );
    structure(
        StructureBuilder::new()
            .add_field("IBusText")
            .add_field(attachments())
            .add_field(text.to_owned())
            .add_field(attributes),
    )
}

/// The text of an `IBusText`, however deeply it is wrapped in variants.
pub fn text_from_value(value: &Value<'_>) -> Option<String> {
    let mut value = value;
    while let Value::Value(inner) = value {
        value = inner;
    }
    let Value::Structure(structure) = value else {
        return None;
    };
    match structure.fields() {
        [Value::Str(name), _, Value::Str(text), ..] if name.as_str() == "IBusText" => {
            Some(text.as_str().to_owned())
        }
        _ => None,
    }
}

/// `IBusPropList`: `(sa{sv}av)`.
fn prop_list_value(properties: Vec<Value<'static>>) -> Value<'static> {
    structure(
        StructureBuilder::new()
            .add_field("IBusPropList")
            .add_field(attachments())
            .add_field(properties),
    )
}

/// `IBusProperty`: `(sa{sv}suvsvbbuvv)`.
fn property_value(
    key: &str,
    kind: u32,
    label: &str,
    symbol: &str,
    state: u32,
    sub_properties: Vec<Value<'static>>,
) -> Value<'static> {
    structure(
        StructureBuilder::new()
            .add_field("IBusProperty")
            .add_field(attachments())
            .add_field(key.to_owned())
            .add_field(kind)
            .add_field(text_value(label))
            .add_field(String::new())
            .add_field(text_value(""))
            .add_field(true)
            .add_field(true)
            .add_field(state)
            .add_field(prop_list_value(sub_properties))
            .add_field(text_value(symbol)),
    )
}

/// Two letters for the indicator: `EN`, `RU`, `ET`.
pub fn symbol(language: Language) -> String {
    language
        .id()
        .chars()
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn language_name(language: Language) -> &'static str {
    match language.id() {
        "en-US" => "English",
        "ru-RU" => "Русский",
        "et-EE" => "Eesti",
        _ => "",
    }
}

/// The indicator (a menu of the layouts), the automatic-conversion switch and the settings item.
pub fn properties_value(shared: &Shared) -> Value<'static> {
    let layout = shared.layout();
    let layouts = shared
        .layouts()
        .iter()
        .map(|language| {
            let name = language_name(*language);
            property_value(
                &format!("{PROP_LAYOUT_PREFIX}{}", language.id()),
                PROP_RADIO,
                if name.is_empty() { language.id() } else { name },
                &symbol(*language),
                if *language == layout {
                    PROP_CHECKED
                } else {
                    PROP_UNCHECKED
                },
                Vec::new(),
            )
        })
        .collect();
    prop_list_value(vec![
        property_value(
            PROP_INPUT_MODE,
            PROP_MENU,
            &tr("menu.layout"),
            &symbol(layout),
            PROP_UNCHECKED,
            layouts,
        ),
        property_value(
            PROP_AUTOMATIC,
            PROP_TOGGLE,
            &tr("menu.automatic"),
            "",
            if shared.automatic() {
                PROP_CHECKED
            } else {
                PROP_UNCHECKED
            },
            Vec::new(),
        ),
        property_value(
            PROP_SETTINGS,
            PROP_NORMAL,
            &tr("menu.settings"),
            "",
            PROP_UNCHECKED,
            Vec::new(),
        ),
    ])
}

/// `IBusEngineDesc`: `(sa{sv}ssssssssussssssss)`.
fn engine_desc_value(version: &str) -> Value<'static> {
    let mut builder = StructureBuilder::new()
        .add_field("IBusEngineDesc")
        .add_field(attachments());
    for field in [
        ENGINE_NAME,
        "AutoKeyboardLayot",
        "Types in English, Russian and Estonian and corrects words typed in the wrong layout",
        "other",
        "MIT",
        "AutoKeyboardLayot",
        "input-keyboard",
        super::engine::BASE_LAYOUT,
    ] {
        builder = builder.add_field(field.to_owned());
    }
    builder = builder.add_field(0u32);
    for field in ["", "⌨", "", "", "", version, "", PROP_INPUT_MODE] {
        builder = builder.add_field(field.to_owned());
    }
    structure(builder)
}

/// `IBusComponent`: `(sa{sv}ssssssssavav)`, for registering a running engine with IBus.
fn component_value(exec: &str, version: &str) -> Value<'static> {
    let mut builder = StructureBuilder::new()
        .add_field("IBusComponent")
        .add_field(attachments());
    for field in [
        COMPONENT_NAME,
        "AutoKeyboardLayot keyboard layout correction",
        version,
        "MIT",
        "AutoKeyboardLayot",
        "https://github.com/woffko/AutoKeyboardLayot",
        exec,
        "",
    ] {
        builder = builder.add_field(field.to_owned());
    }
    structure(
        builder
            .add_field(Vec::<Value<'static>>::new())
            .add_field(vec![engine_desc_value(version)]),
    )
}

/// The component file IBus reads from its component directory.
pub fn component_xml(exec: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<component>
  <name>{COMPONENT_NAME}</name>
  <description>AutoKeyboardLayot keyboard layout correction</description>
  <exec>{exec}</exec>
  <version>{version}</version>
  <author>AutoKeyboardLayot</author>
  <license>MIT</license>
  <homepage>https://github.com/woffko/AutoKeyboardLayot</homepage>
  <textdomain></textdomain>
  <engines>
    <engine>
      <name>{ENGINE_NAME}</name>
      <language>other</language>
      <license>MIT</license>
      <author>AutoKeyboardLayot</author>
      <icon>input-keyboard</icon>
      <layout>{layout}</layout>
      <longname>AutoKeyboardLayot</longname>
      <description>Types in English, Russian and Estonian and corrects words typed in the wrong layout</description>
      <rank>0</rank>
      <symbol>⌨</symbol>
      <icon_prop_key>{PROP_INPUT_MODE}</icon_prop_key>
    </engine>
  </engines>
</component>
"#,
        exec = xml_escape(exec),
        version = xml_escape(version),
        layout = super::engine::BASE_LAYOUT,
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Registers this running process as the engine's component (for development and tests; an
/// installed engine is started by IBus from its component file and only claims its name).
pub fn register_component(connection: &Connection, exec: &str, version: &str) -> zbus::Result<()> {
    connection.call_method(
        Some(IBUS_SERVICE),
        IBUS_PATH,
        Some(IBUS_SERVICE),
        "RegisterComponent",
        &(component_value(exec, version),),
    )?;
    Ok(())
}

/// Gives a newer shared state when the configuration changed.
type Reload = Box<dyn FnMut(&Shared) -> Option<Shared>>;

/// What the engine objects run on: the shared state and one engine per input context.
pub struct Host {
    shared: Shared,
    engines: Vec<(String, Engine)>,
    next_engine: u32,
    /// Called on focus-in: a newer configuration replaces the shared state.
    reload: Option<Reload>,
    /// Called for the settings item of the menu.
    open_settings: Option<Box<dyn FnMut()>>,
}

impl Host {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            engines: Vec::new(),
            next_engine: 1,
            reload: None,
            open_settings: None,
        }
    }

    pub fn with_reload(mut self, reload: impl FnMut(&Shared) -> Option<Shared> + 'static) -> Self {
        self.reload = Some(Box::new(reload));
        self
    }

    pub fn with_settings(mut self, open: impl FnMut() + 'static) -> Self {
        self.open_settings = Some(Box::new(open));
        self
    }

    /// Serves calls until the connection closes. `announce` makes the engine known to IBus (takes the
    /// component's bus name or registers the component). It runs once the engine listens: IBus asks
    /// the factory for an engine as soon as it learns of it, and a call that comes before anyone
    /// listens is lost.
    pub fn serve(
        &mut self,
        connection: &Connection,
        announce: impl FnOnce() -> zbus::Result<()>,
    ) -> zbus::Result<()> {
        let messages = MessageIterator::from(connection);
        announce()?;
        for message in messages {
            let message = message?;
            if message.message_type() == MessageType::MethodCall {
                self.dispatch(connection, &message)?;
            }
        }
        Ok(())
    }

    fn engine_index(&self, path: &str) -> Option<usize> {
        self.engines
            .iter()
            .position(|(candidate, _)| candidate == path)
    }

    fn dispatch(&mut self, connection: &Connection, message: &Message) -> zbus::Result<()> {
        let header = message.header();
        let path = header
            .path()
            .map(|path| path.as_str().to_owned())
            .unwrap_or_default();
        let interface = header
            .interface()
            .map(|name| name.as_str().to_owned())
            .unwrap_or_default();
        let member = header
            .member()
            .map(|name| name.as_str().to_owned())
            .unwrap_or_default();
        if interface == INTROSPECTABLE_INTERFACE && member == "Introspect" {
            return connection.reply(&header, &("<node/>",));
        }
        if path == FACTORY_PATH {
            return match (interface.as_str(), member.as_str()) {
                (FACTORY_INTERFACE, "CreateEngine") => {
                    let name: String = message.body().deserialize()?;
                    if name != ENGINE_NAME {
                        return connection.reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.InvalidArgs",
                            &(format!("unknown engine {name}"),),
                        );
                    }
                    let engine_path = format!("{ENGINE_PATH_PREFIX}{}", self.next_engine);
                    self.next_engine += 1;
                    self.engines
                        .push((engine_path.clone(), Engine::new(&self.shared)));
                    connection.reply(&header, &(ObjectPath::try_from(engine_path.as_str())?,))
                }
                (SERVICE_INTERFACE, "Destroy") => connection.reply(&header, &()),
                _ => unknown(connection, message),
            };
        }
        let Some(index) = self.engine_index(&path) else {
            return unknown(connection, message);
        };
        match interface.as_str() {
            ENGINE_INTERFACE => self.engine_call(connection, message, index, &path, &member),
            SERVICE_INTERFACE if member == "Destroy" => {
                self.engines.remove(index);
                connection.reply(&header, &())
            }
            PROPERTIES_INTERFACE => self.property_call(connection, message, index, &member),
            _ => unknown(connection, message),
        }
    }

    fn engine_call(
        &mut self,
        connection: &Connection,
        message: &Message,
        index: usize,
        path: &str,
        member: &str,
    ) -> zbus::Result<()> {
        let header = message.header();
        match member {
            "ProcessKeyEvent" => {
                let (keyval, keycode, state): (u32, u32, u32) = message.body().deserialize()?;
                let engine = &mut self.engines[index].1;
                let outcome = engine.process_key(&mut self.shared, keyval, keycode, state);
                self.emit_actions(connection, path, outcome.actions)?;
                connection.reply(&header, &(outcome.handled,))
            }
            "FocusIn" | "FocusInId" => {
                self.maybe_reload();
                let actions = self.engines[index].1.focus_in(&self.shared);
                self.emit_actions(connection, path, actions)?;
                connection.reply(&header, &())
            }
            "FocusOut" | "FocusOutId" | "Disable" => {
                self.engines[index].1.focus_out();
                connection.reply(&header, &())
            }
            "Reset" => {
                self.engines[index].1.reset();
                connection.reply(&header, &())
            }
            "Enable" => {
                emit(
                    connection,
                    path,
                    "RegisterProperties",
                    &(properties_value(&self.shared),),
                )?;
                connection.reply(&header, &())
            }
            "SetSurroundingText" => {
                let body = message.body();
                let (text, cursor, anchor): (Value<'_>, u32, u32) = body.deserialize()?;
                if let Some(text) = text_from_value(&text) {
                    self.engines[index]
                        .1
                        .set_surrounding_text(text, cursor, anchor);
                }
                connection.reply(&header, &())
            }
            "PropertyActivate" => {
                let (name, state): (String, u32) = message.body().deserialize()?;
                self.activate_property(connection, path, &name, state)?;
                connection.reply(&header, &())
            }
            "SetCapabilities"
            | "SetCursorLocation"
            | "SetCursorLocationRelative"
            | "PageUp"
            | "PageDown"
            | "CursorUp"
            | "CursorDown"
            | "CandidateClicked"
            | "PropertyShow"
            | "PropertyHide"
            | "ProcessHandWritingEvent"
            | "CancelHandWriting"
            | "PanelExtensionReceived"
            | "PanelExtensionRegisterKeys" => connection.reply(&header, &()),
            _ => unknown(connection, message),
        }
    }

    fn property_call(
        &mut self,
        connection: &Connection,
        message: &Message,
        index: usize,
        member: &str,
    ) -> zbus::Result<()> {
        let header = message.header();
        match member {
            "Set" => {
                let body = message.body();
                let (_interface, name, value): (String, String, Value<'_>) = body.deserialize()?;
                if name == "ContentType" {
                    let mut value = &value;
                    while let Value::Value(inner) = value {
                        value = inner;
                    }
                    if let Value::Structure(structure) = value
                        && let [Value::U32(purpose), Value::U32(hints)] = structure.fields()
                    {
                        let (purpose, hints) = (*purpose, *hints);
                        self.engines[index]
                            .1
                            .set_content_type(&self.shared, purpose, hints);
                    }
                }
                connection.reply(&header, &())
            }
            "Get" => {
                let (_interface, name): (String, String) = message.body().deserialize()?;
                // IBus reads both flags as plain booleans, although its introspection declares
                // "(b)"; a structure makes it log a GLib warning and take the flag as false.
                match name.as_str() {
                    // FocusIn and FocusOut, not their variants with an input context path.
                    "FocusId" => connection.reply(&header, &(Value::Bool(false),)),
                    "ActiveSurroundingText" => connection.reply(&header, &(Value::Bool(true),)),
                    _ => unknown(connection, message),
                }
            }
            "GetAll" => {
                let mut all = HashMap::new();
                all.insert("FocusId".to_owned(), Value::Bool(false));
                all.insert("ActiveSurroundingText".to_owned(), Value::Bool(true));
                connection.reply(&header, &(all,))
            }
            _ => unknown(connection, message),
        }
    }

    fn activate_property(
        &mut self,
        connection: &Connection,
        path: &str,
        name: &str,
        state: u32,
    ) -> zbus::Result<()> {
        if name == PROP_AUTOMATIC {
            self.shared.set_automatic(state == PROP_CHECKED);
        } else if name == PROP_SETTINGS {
            if let Some(open) = &mut self.open_settings {
                open();
            }
            return Ok(());
        } else if let Some(id) = name.strip_prefix(PROP_LAYOUT_PREFIX) {
            let chosen = self
                .shared
                .layouts()
                .iter()
                .copied()
                .find(|language| language.id() == id);
            if let (Some(language), PROP_CHECKED) = (chosen, state) {
                self.engines
                    .iter_mut()
                    .for_each(|(_, engine)| engine.reset());
                self.shared.choose_layout(language);
            }
        } else {
            return Ok(());
        }
        emit(
            connection,
            path,
            "RegisterProperties",
            &(properties_value(&self.shared),),
        )
    }

    fn maybe_reload(&mut self) {
        let Some(reload) = &mut self.reload else {
            return;
        };
        if let Some(mut replacement) = reload(&self.shared) {
            replacement.carry_over(&self.shared);
            self.shared = replacement;
            for (_, engine) in &mut self.engines {
                *engine = Engine::new(&self.shared);
            }
        }
    }

    fn emit_actions(
        &self,
        connection: &Connection,
        path: &str,
        actions: Vec<Action>,
    ) -> zbus::Result<()> {
        for action in actions {
            match action {
                Action::Commit(text) => {
                    emit(connection, path, "CommitText", &(text_value(&text),))?
                }
                Action::ForwardKey {
                    keyval,
                    keycode,
                    state,
                } => {
                    emit(
                        connection,
                        path,
                        "ForwardKeyEvent",
                        &(keyval, keycode, state),
                    )?;
                }
                Action::DeleteSurrounding { offset, chars } => {
                    emit(connection, path, "DeleteSurroundingText", &(offset, chars))?;
                }
                Action::Layout(_) => {
                    emit(
                        connection,
                        path,
                        "RegisterProperties",
                        &(properties_value(&self.shared),),
                    )?;
                }
                Action::RequireSurrounding => {
                    emit(connection, path, "RequireSurroundingText", &())?
                }
            }
        }
        Ok(())
    }
}

fn emit<B>(connection: &Connection, path: &str, member: &str, body: &B) -> zbus::Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let message = Message::signal(path, ENGINE_INTERFACE, member)?.build(body)?;
    connection.send(&message)
}

fn unknown(connection: &Connection, message: &Message) -> zbus::Result<()> {
    let header = message.header();
    let member = header
        .member()
        .map(|name| name.as_str().to_owned())
        .unwrap_or_default();
    connection.reply_error(
        &header,
        "org.freedesktop.DBus.Error.UnknownMethod",
        &(format!("{member} is not supported"),),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_have_the_signatures_ibus_expects() {
        assert_eq!(text_value("x").value_signature().to_string(), "(sa{sv}sv)");
        assert_eq!(
            text_from_value(&text_value("привет")).as_deref(),
            Some("привет")
        );
        assert_eq!(
            text_from_value(&Value::Value(Box::new(text_value("a")))).as_deref(),
            Some("a")
        );
        assert_eq!(text_from_value(&Value::from("plain")), None);
        assert_eq!(
            property_value("k", PROP_NORMAL, "l", "s", 0, Vec::new())
                .value_signature()
                .to_string(),
            "(sa{sv}suvsvbbuvv)"
        );
        assert_eq!(
            prop_list_value(Vec::new()).value_signature().to_string(),
            "(sa{sv}av)"
        );
        assert_eq!(
            engine_desc_value("1").value_signature().to_string(),
            "(sa{sv}ssssssssussssssss)"
        );
        assert_eq!(
            component_value("/bin/true", "1")
                .value_signature()
                .to_string(),
            "(sa{sv}ssssssssavav)"
        );
    }

    #[test]
    fn the_component_file_names_the_engine_and_its_keymap() {
        let xml = component_xml("/usr/bin/autokeyboardlayot --ibus", "0.1.1");
        assert!(xml.contains("<name>org.freedesktop.IBus.AutoKeyboardLayot</name>"));
        assert!(xml.contains("<exec>/usr/bin/autokeyboardlayot --ibus</exec>"));
        assert!(xml.contains("<layout>us</layout>"));
        assert_eq!(xml_escape("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
        assert_eq!(symbol(Language::Estonian), "ET");
    }
}
