//! UI copy in Russian and English.

pub struct Strings {
    pub about: &'static str,
    pub settings: &'static str,
    pub store: &'static str,
    pub force_quit: &'static str,
    pub sleep: &'static str,
    pub restart: &'static str,
    pub restart_now: &'static str,
    pub shut_down: &'static str,
    pub shut_down_now: &'static str,
    pub lock: &'static str,
    pub log_out: &'static str,
    pub log_out_now: &'static str,
    pub cancel: &'static str,
    pub hide: &'static str,
    pub hide_others: &'static str,
    pub show_all: &'static str,
    pub quit: &'static str,
    pub window: &'static str,
    pub minimize: &'static str,
    pub zoom: &'static str,
    pub tile_left: &'static str,
    pub tile_right: &'static str,
    pub center: &'static str,
    pub bring_all: &'static str,
    pub desktop: &'static str,
    pub open: &'static str,
    pub keep_in_dock: &'static str,
    pub remove_from_dock: &'static str,
    pub show_in_folder: &'static str,
    pub new_window: &'static str,
    pub trash: &'static str,
    pub empty_trash: &'static str,
    pub downloads: &'static str,
    pub search_placeholder: &'static str,
    pub kind_app: &'static str,
    pub kind_setting: &'static str,
    pub kind_folder: &'static str,
    pub kind_file: &'static str,
    pub kind_calc: &'static str,
    pub kind_web: &'static str,
    pub web_search: &'static str,
    pub copied: &'static str,
    pub no_results: &'static str,
    pub quit_shell: &'static str,
    pub calc_hint: &'static str,
    pub days: [&'static str; 7],
    pub months: [&'static str; 12],
}

impl Strings {
    pub const RU: Strings = Strings {
        about: "Об этом компьютере",
        settings: "Системные настройки…",
        store: "Microsoft Store…",
        force_quit: "Завершить принудительно…",
        sleep: "Режим сна",
        restart: "Перезагрузить…",
        restart_now: "Перезагрузить сейчас",
        shut_down: "Выключить…",
        shut_down_now: "Выключить сейчас",
        lock: "Заблокировать экран",
        log_out: "Завершить сеанс…",
        log_out_now: "Завершить сеанс сейчас",
        cancel: "Отменить",
        hide: "Скрыть",
        hide_others: "Скрыть остальные",
        show_all: "Показать все",
        quit: "Завершить",
        window: "Окно",
        minimize: "Свернуть",
        zoom: "Изменить масштаб",
        tile_left: "Поместить окно слева",
        tile_right: "Поместить окно справа",
        center: "Расположить по центру",
        bring_all: "Все окна — на передний план",
        desktop: "Рабочий стол",
        open: "Открыть",
        keep_in_dock: "Оставить в Dock",
        remove_from_dock: "Удалить из Dock",
        show_in_folder: "Показать в Проводнике",
        new_window: "Новое окно",
        trash: "Корзина",
        empty_trash: "Очистить Корзину",
        downloads: "Загрузки",
        search_placeholder: "Поиск",
        kind_app: "Программа",
        kind_setting: "Параметры",
        kind_folder: "Папка",
        kind_file: "Документ",
        kind_calc: "Калькулятор",
        kind_web: "Интернет",
        web_search: "Искать в интернете",
        copied: "Скопировано",
        no_results: "Нет результатов",
        quit_shell: "Завершить macRust",
        calc_hint: "Enter — скопировать результат",
        days: ["Вс", "Пн", "Вт", "Ср", "Чт", "Пт", "Сб"],
        months: ["янв.", "февр.", "мар.", "апр.", "мая", "июн.", "июл.", "авг.", "сент.", "окт.", "нояб.", "дек."],
    };

    pub const EN: Strings = Strings {
        about: "About This PC",
        settings: "System Settings…",
        store: "Microsoft Store…",
        force_quit: "Force Quit…",
        sleep: "Sleep",
        restart: "Restart…",
        restart_now: "Restart Now",
        shut_down: "Shut Down…",
        shut_down_now: "Shut Down Now",
        lock: "Lock Screen",
        log_out: "Log Out…",
        log_out_now: "Log Out Now",
        cancel: "Cancel",
        hide: "Hide",
        hide_others: "Hide Others",
        show_all: "Show All",
        quit: "Quit",
        window: "Window",
        minimize: "Minimize",
        zoom: "Zoom",
        tile_left: "Move Window to Left Side",
        tile_right: "Move Window to Right Side",
        center: "Center",
        bring_all: "Bring All to Front",
        desktop: "Desktop",
        open: "Open",
        keep_in_dock: "Keep in Dock",
        remove_from_dock: "Remove from Dock",
        show_in_folder: "Show in Explorer",
        new_window: "New Window",
        trash: "Recycle Bin",
        empty_trash: "Empty Recycle Bin",
        downloads: "Downloads",
        search_placeholder: "Search",
        kind_app: "Application",
        kind_setting: "Settings",
        kind_folder: "Folder",
        kind_file: "Document",
        kind_calc: "Calculator",
        kind_web: "Web",
        web_search: "Search the web for",
        copied: "Copied",
        no_results: "No results",
        quit_shell: "Quit macRust",
        calc_hint: "Press Enter to copy the result",
        days: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
        months: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    };

    /// Menu-bar clock: «Пт 21 нояб. 14:32» / «Fri Nov 21 14:32».
    pub fn is_ru(&self) -> bool {
        self.days[0] == "Вс"
    }

    /// «Скрыть «Блокнот»» / «Hide Notepad».
    pub fn with_app(&self, label: &str, app: &str) -> String {
        if app.is_empty() {
            label.to_string()
        } else if self.is_ru() {
            format!("{label} «{app}»")
        } else {
            format!("{label} {app}")
        }
    }

    pub fn clock(&self, weekday: usize, day: u32, month: usize, hour: u32, minute: u32) -> String {
        let wd = self.days[weekday % 7];
        let mo = self.months[month.min(11)];
        if !self.is_ru() {
            format!("{wd} {mo} {day} {hour:02}:{minute:02}")
        } else {
            format!("{wd} {day} {mo} {hour:02}:{minute:02}")
        }
    }
}
