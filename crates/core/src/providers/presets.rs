//! Пресеты категорий: текст запроса для Google/Яндекса и теги OpenStreetMap.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    /// Запрос для текстового поиска Google и Яндекса в русскоязычных странах.
    pub query: &'static str,
    /// Запрос для остальных стран (Google понимает английский запрос везде).
    pub query_en: &'static str,
    /// Пары тег=значение OpenStreetMap.
    pub osm: &'static [(&'static str, &'static str)],
    /// Основы слов ниши на разных языках: ищем их в title/H1/тексте (F06, N01).
    #[serde(skip)]
    pub keywords: &'static [&'static str],
    /// Короткие суффиксы для угадывания домена: «ulybka-dent.ru».
    #[serde(skip)]
    pub domain_suffixes: &'static [&'static str],
}

pub const PRESETS: &[Preset] = &[
    Preset { id: "dentist", label: "Стоматология", query: "стоматология", query_en: "dentist", osm: &[("amenity", "dentist"), ("healthcare", "dentist")],
        keywords: &["стомат", "зуб", "dentist", "dental", "zahnarzt", "zahn", "dentaire", "dentiste", "dentista", "dental", "odontolog", "tandarts", "stomatolog", "dentysta"],
        domain_suffixes: &["dent", "dental", "stom"] },
    Preset { id: "clinic", label: "Медицинский центр", query: "медицинский центр", query_en: "medical clinic", osm: &[("amenity", "clinic"), ("amenity", "doctors"), ("healthcare", "clinic")],
        keywords: &["клиник", "медиц", "врач", "clinic", "medical", "doctor", "health", "praxis", "arzt", "ärzt", "médic", "medic", "clínica", "clinica", "poliklinik", "przychodnia"],
        domain_suffixes: &["med", "clinic"] },
    Preset { id: "vet", label: "Ветеринарная клиника", query: "ветеринарная клиника", query_en: "veterinary clinic", osm: &[("amenity", "veterinary")],
        keywords: &["ветерин", "vet", "tierarzt", "tierärzt", "vétérinaire", "veterinari", "dierenarts", "weteryn"],
        domain_suffixes: &["vet"] },
    Preset { id: "beauty", label: "Салон красоты", query: "салон красоты", query_en: "beauty salon", osm: &[("shop", "beauty")],
        keywords: &["красот", "салон", "маникюр", "косметолог", "beauty", "salon", "nail", "spa", "kosmetik", "beauté", "institut", "belleza", "estética", "estetica", "bellezza", "schoonheid", "uroda"],
        domain_suffixes: &["beauty", "salon"] },
    Preset { id: "hairdresser", label: "Парикмахерская, барбершоп", query: "парикмахерская", query_en: "hair salon", osm: &[("shop", "hairdresser")],
        keywords: &["парикмахер", "барбер", "стрижк", "hair", "barber", "salon", "friseur", "coiffeur", "coiffure", "peluquer", "barbería", "parrucch", "kapper", "fryzjer", "braid"],
        domain_suffixes: &["salon", "hair", "barber"] },
    Preset { id: "fitness", label: "Фитнес-клуб", query: "фитнес клуб", query_en: "gym", osm: &[("leisure", "fitness_centre"), ("leisure", "sports_centre")],
        keywords: &["фитнес", "тренаж", "спорт", "fitness", "gym", "training", "sport", "yoga", "pilates", "crossfit", "gimnasio", "palestra", "siłownia"],
        domain_suffixes: &["fitness", "gym"] },
    Preset { id: "car_repair", label: "Автосервис", query: "автосервис", query_en: "car repair", osm: &[("shop", "car_repair")],
        keywords: &["автосервис", "ремонт авто", "сто ", "auto", "car repair", "mechanic", "garage", "kfz", "werkstatt", "garage", "taller", "officina", "autoservice", "warsztat"],
        domain_suffixes: &["auto", "service"] },
    Preset { id: "tyres", label: "Шиномонтаж", query: "шиномонтаж", query_en: "tire shop", osm: &[("shop", "tyres")],
        keywords: &["шиномонтаж", "шин", "tire", "tyre", "reifen", "pneu", "neumátic", "gomm", "banden", "opony", "wulkaniz"],
        domain_suffixes: &["tyres", "shina"] },
    Preset { id: "car_wash", label: "Автомойка", query: "автомойка", query_en: "car wash", osm: &[("amenity", "car_wash")],
        keywords: &["мойк", "car wash", "autowäsche", "waschanlage", "lavage", "lavado", "autolavaggio", "myjnia", "wasstraat"],
        domain_suffixes: &["wash"] },
    Preset { id: "cafe", label: "Кафе", query: "кафе", query_en: "cafe", osm: &[("amenity", "cafe")],
        keywords: &["кафе", "кофе", "cafe", "café", "coffee", "kaffee", "caffè", "cafetería", "kawiarnia", "koffie", "bakery", "bäckerei"],
        domain_suffixes: &["cafe", "coffee"] },
    Preset { id: "restaurant", label: "Ресторан", query: "ресторан", query_en: "restaurant", osm: &[("amenity", "restaurant")],
        keywords: &["ресторан", "кухн", "меню", "restaurant", "kitchen", "menu", "grill", "bistro", "trattoria", "ristorante", "restaurante", "pizzeria", "speisekarte", "restauracja"],
        domain_suffixes: &["restaurant", "rest"] },
    Preset { id: "hotel", label: "Гостиница", query: "гостиница", query_en: "hotel", osm: &[("tourism", "hotel"), ("tourism", "guest_house"), ("tourism", "hostel")],
        keywords: &["гостиниц", "отель", "хостел", "hotel", "hostel", "inn", "lodge", "pension", "gasthaus", "hôtel", "albergo", "hostal", "b&b"],
        domain_suffixes: &["hotel"] },
    Preset { id: "lawyer", label: "Юридические услуги", query: "юридические услуги", query_en: "lawyer", osm: &[("office", "lawyer")],
        keywords: &["юрид", "юрист", "адвокат", "lawyer", "attorney", "law firm", "legal", "rechtsanwalt", "anwalt", "kanzlei", "avocat", "abogado", "avvocat", "advocaat", "adwokat", "solicitor"],
        domain_suffixes: &["law", "legal"] },
    Preset { id: "notary", label: "Нотариус", query: "нотариус", query_en: "notary", osm: &[("office", "notary")],
        keywords: &["нотари", "notar", "notaire", "notario", "notaio", "notaris", "notariusz"],
        domain_suffixes: &["notary", "notarius"] },
    Preset { id: "estate_agent", label: "Агентство недвижимости", query: "агентство недвижимости", query_en: "real estate agency", osm: &[("office", "estate_agent")],
        keywords: &["недвижим", "риелт", "риэлт", "real estate", "realty", "realtor", "homes", "immobilien", "makler", "immobilier", "inmobiliaria", "immobiliare", "makelaar", "nieruchomo"],
        domain_suffixes: &["realty", "estate"] },
    Preset { id: "florist", label: "Цветы", query: "цветы", query_en: "florist", osm: &[("shop", "florist")],
        keywords: &["цвет", "букет", "флорист", "flower", "florist", "blumen", "fleur", "flores", "floristería", "fiori", "fiorista", "bloem", "kwiat"],
        domain_suffixes: &["flowers", "flora"] },
    Preset { id: "electronics_repair", label: "Ремонт техники", query: "ремонт телефонов", query_en: "phone repair", osm: &[("craft", "electronics_repair"), ("shop", "mobile_phone")],
        keywords: &["ремонт", "телефон", "смартфон", "repair", "phone", "iphone", "reparatur", "handy", "réparation", "reparación", "riparazione", "reparatie", "naprawa"],
        domain_suffixes: &["repair", "service"] },
    Preset { id: "dry_cleaning", label: "Химчистка", query: "химчистка", query_en: "dry cleaner", osm: &[("shop", "dry_cleaning"), ("shop", "laundry")],
        keywords: &["химчист", "прачечн", "dry clean", "laundry", "cleaners", "reinigung", "wäscherei", "pressing", "blanchisserie", "tintorería", "lavandería", "lavanderia", "tintoria", "stomerij", "pralnia"],
        domain_suffixes: &["clean"] },
    Preset { id: "optician", label: "Оптика", query: "оптика", query_en: "optician", osm: &[("shop", "optician")],
        keywords: &["оптик", "очк", "линз", "optic", "optician", "optometr", "eyewear", "glasses", "optiker", "brillen", "opticien", "lunettes", "óptica", "ottica", "opticien", "optyk"],
        domain_suffixes: &["optic", "optika"] },
    Preset { id: "language_school", label: "Языковая школа", query: "языковая школа", query_en: "language school", osm: &[("amenity", "language_school")],
        keywords: &["язык", "английск", "школа", "language", "english", "school", "sprachschule", "sprachen", "école de langues", "langues", "idiomas", "academia", "lingue", "taalschool", "językow"],
        domain_suffixes: &["school", "english"] },
    Preset { id: "driving_school", label: "Автошкола", query: "автошкола", query_en: "driving school", osm: &[("amenity", "driving_school")],
        keywords: &["автошкол", "вожден", "driving", "fahrschule", "auto-école", "autoescuela", "autoscuola", "rijschool", "szkoła jazdy", "nauka jazdy"],
        domain_suffixes: &["avtoshkola", "driving"] },
    Preset { id: "furniture", label: "Мебель", query: "мебель", query_en: "furniture store", osm: &[("shop", "furniture")],
        keywords: &["мебел", "furniture", "sofa", "möbel", "meubles", "muebles", "mobili", "meubel", "meble"],
        domain_suffixes: &["mebel", "furniture"] },
    Preset { id: "hardware", label: "Стройматериалы", query: "строительные материалы", query_en: "hardware store", osm: &[("shop", "hardware"), ("shop", "doityourself")],
        keywords: &["строит", "стройматериал", "инструмент", "hardware", "building", "tools", "lumber", "baumarkt", "baustoffe", "eisenwaren", "quincaillerie", "bricolage", "ferretería", "ferramenta", "bouwmarkt", "budowlan"],
        domain_suffixes: &["stroy", "build"] },
];

pub fn find(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Ниши, где онлайн-запись — норма (N04 строже) и нужна лицензия (M06).
pub fn is_medical(query: &str) -> bool {
    let q = query.to_lowercase();
    ["стомат", "клиник", "медиц", "врач", "ветерин", "dentist", "dental", "clinic", "medical", "doctor", "veterin", "zahnarzt", "arzt", "praxis"]
        .iter()
        .any(|k| q.contains(k))
}

/// «Все организации»: какие точки OSM считаем бизнесом. Ключи целиком (любой магазин,
/// мастерская, офис, медицина) и выбранные значения там, где под ключом много не-бизнеса
/// (amenity — это и скамейки, и парковки).
pub const ALL_KEYS: &[&str] = &["shop", "craft", "healthcare"];
/// Офисы, кроме государственных и общественных.
pub const OFFICE_EXCLUDE: &str = "government|ngo|religion|political_party|diplomatic|association|educational_institution|quango|union|foundation";
pub const ALL_AMENITY: &str = "restaurant|cafe|fast_food|bar|pub|biergarten|ice_cream|food_court|dentist|clinic|doctors|pharmacy|veterinary|\
car_wash|car_rental|car_repair|vehicle_inspection|fuel|charging_station|driving_school|language_school|music_school|dancing_school|\
training|prep_school|kindergarten|childcare|cinema|theatre|nightclub|studio|events_venue|internet_cafe|coworking_space|\
marketplace|bank|bureau_de_change|money_transfer|post_office|beauty|spa|massage|sauna|photo_booth|veterinary_pharmacy";
pub const ALL_LEISURE: &str = "fitness_centre|sports_centre|dance|sauna|escape_game|bowling_alley|amusement_arcade|trampoline_park|\
swimming_pool|ice_rink|climbing|horse_riding|golf_course|water_park|hackerspace|tanning_salon";
pub const ALL_TOURISM: &str = "hotel|guest_house|hostel|apartment|motel|chalet|camp_site|gallery|museum";

/// Русское название вида точки OSM для колонки «категория».
pub fn osm_label(key: &str, value: &str) -> Option<&'static str> {
    Some(match (key, value) {
        ("amenity", "restaurant") => "Ресторан",
        ("amenity", "cafe") => "Кафе",
        ("amenity", "fast_food") => "Фастфуд",
        ("amenity", "bar") | ("amenity", "pub") => "Бар",
        ("amenity", "ice_cream") => "Мороженое",
        ("amenity", "dentist") | ("healthcare", "dentist") => "Стоматология",
        ("amenity", "clinic") | ("healthcare", "clinic") => "Клиника",
        ("amenity", "doctors") | ("healthcare", "doctor") => "Врач",
        ("amenity", "pharmacy") | ("healthcare", "pharmacy") => "Аптека",
        ("amenity", "veterinary") => "Ветклиника",
        ("amenity", "car_wash") => "Автомойка",
        ("amenity", "car_rental") => "Прокат авто",
        ("amenity", "car_repair") | ("shop", "car_repair") => "Автосервис",
        ("amenity", "fuel") => "АЗС",
        ("amenity", "driving_school") => "Автошкола",
        ("amenity", "language_school") => "Языковая школа",
        ("amenity", "music_school") => "Музыкальная школа",
        ("amenity", "dancing_school") | ("leisure", "dance") => "Танцы",
        ("amenity", "training") | ("amenity", "prep_school") => "Курсы",
        ("amenity", "kindergarten") | ("amenity", "childcare") => "Детский сад",
        ("amenity", "cinema") => "Кино",
        ("amenity", "theatre") => "Театр",
        ("amenity", "nightclub") => "Ночной клуб",
        ("amenity", "studio") => "Студия",
        ("amenity", "events_venue") => "Площадка для событий",
        ("amenity", "coworking_space") => "Коворкинг",
        ("amenity", "marketplace") => "Рынок",
        ("amenity", "bank") => "Банк",
        ("amenity", "bureau_de_change") => "Обмен валют",
        ("amenity", "post_office") => "Почта",
        ("amenity", "spa") | ("leisure", "sauna") | ("amenity", "sauna") => "Баня, спа",
        ("amenity", "massage") | ("shop", "massage") => "Массаж",
        ("leisure", "fitness_centre") => "Фитнес",
        ("leisure", "sports_centre") => "Спортцентр",
        ("leisure", "escape_game") => "Квест",
        ("leisure", "swimming_pool") => "Бассейн",
        ("leisure", "tanning_salon") | ("shop", "tanning") => "Солярий",
        ("tourism", "hotel") | ("tourism", "motel") => "Гостиница",
        ("tourism", "guest_house") => "Гостевой дом",
        ("tourism", "hostel") => "Хостел",
        ("tourism", "apartment") => "Апартаменты",
        ("tourism", "gallery") => "Галерея",
        ("tourism", "museum") => "Музей",
        ("healthcare", "hospital") | ("amenity", "hospital") => "Больница",
        ("healthcare", "laboratory") => "Лаборатория",
        ("healthcare", "physiotherapist") => "Физиотерапия",
        ("healthcare", "psychotherapist") => "Психотерапевт",
        ("healthcare", "optometrist") | ("shop", "optician") => "Оптика",
        ("shop", "beauty") => "Салон красоты",
        ("shop", "hairdresser") => "Парикмахерская",
        ("shop", "cosmetics") | ("shop", "perfumery") => "Косметика",
        ("shop", "supermarket") => "Супермаркет",
        ("shop", "convenience") => "Продукты",
        ("shop", "bakery") | ("shop", "pastry") | ("shop", "confectionery") => "Выпечка, сладости",
        ("shop", "butcher") => "Мясо",
        ("shop", "alcohol") | ("shop", "wine") | ("shop", "beverages") => "Напитки",
        ("shop", "clothes") | ("shop", "boutique") | ("shop", "fashion") => "Одежда",
        ("shop", "shoes") => "Обувь",
        ("shop", "jewelry") => "Ювелирный",
        ("shop", "florist") => "Цветы",
        ("shop", "gift") => "Подарки",
        ("shop", "books") => "Книги",
        ("shop", "stationery") => "Канцтовары",
        ("shop", "toys") => "Игрушки",
        ("shop", "sports") => "Спорттовары",
        ("shop", "mobile_phone") => "Телефоны",
        ("shop", "electronics") | ("shop", "computer") => "Электроника",
        ("shop", "furniture") => "Мебель",
        ("shop", "hardware") | ("shop", "doityourself") => "Стройматериалы",
        ("shop", "pet") => "Зоотовары",
        ("shop", "tobacco") | ("shop", "e-cigarette") => "Табак",
        ("shop", "kiosk") => "Киоск",
        ("shop", "copyshop") => "Копицентр",
        ("shop", "dry_cleaning") | ("shop", "laundry") => "Химчистка",
        ("shop", "tyres") => "Шиномонтаж",
        ("shop", "car") => "Автосалон",
        ("shop", "car_parts") => "Автозапчасти",
        ("shop", "travel_agency") | ("office", "travel_agent") => "Турагентство",
        ("shop", "tailor") | ("craft", "tailor") => "Ателье",
        ("shop", "tattoo") => "Тату",
        ("shop", "nutrition_supplements") => "Спортпит",
        ("shop", "medical_supply") => "Медтехника",
        ("shop", "variety_store") => "Всё по одной цене",
        ("shop", "department_store") | ("shop", "mall") => "Торговый центр",
        ("shop", "second_hand") => "Секонд-хенд",
        ("shop", "bag") => "Сумки",
        ("shop", "watches") => "Часы",
        ("shop", "photo") => "Фото",
        ("shop", "music") | ("shop", "musical_instrument") => "Музыка",
        ("shop", "bicycle") => "Велосипеды",
        ("shop", "fabric") => "Ткани",
        ("shop", "interior_decoration") => "Декор",
        ("shop", "lighting") => "Свет",
        ("shop", "kitchen") => "Кухни",
        ("shop", "bed") => "Матрасы",
        ("shop", "houseware") => "Товары для дома",
        ("shop", "erotic") => "Интим",
        ("shop", "pawnbroker") => "Ломбард",
        ("shop", "repair") | ("craft", "electronics_repair") => "Ремонт",
        ("shop", "funeral_directors") => "Ритуальные услуги",
        ("office", "lawyer") => "Юрист",
        ("office", "notary") => "Нотариус",
        ("office", "estate_agent") => "Недвижимость",
        ("office", "accountant") | ("office", "tax_advisor") => "Бухгалтерия",
        ("office", "insurance") => "Страхование",
        ("office", "it") => "IT-компания",
        ("office", "company") => "Компания",
        ("office", "architect") => "Архитектор",
        ("office", "advertising_agency") => "Реклама",
        ("office", "employment_agency") => "Кадровое агентство",
        ("office", "financial") | ("office", "financial_advisor") => "Финансы",
        ("office", "logistics") | ("office", "moving_company") => "Логистика",
        ("office", "educational_institution") => "Обучение",
        ("craft", "photographer") => "Фотограф",
        ("craft", "shoemaker") => "Ремонт обуви",
        ("craft", "key_cutter") | ("shop", "locksmith") => "Ключи",
        ("craft", "printer") => "Типография",
        ("craft", "carpenter") => "Столярка",
        ("craft", "electrician") => "Электрик",
        ("craft", "plumber") => "Сантехник",
        ("craft", "jeweller") => "Ювелир",
        _ => return None,
    })
}

/// Подстроки категорий Overture (`taxonomy.primary` и `basic_category`) для пресета.
pub fn overture_patterns(id: &str) -> &'static [&'static str] {
    match id {
        "dentist" => &["dent", "orthodont"],
        "clinic" => &["clinic", "doctor", "medical", "outpatient", "health_care", "diagnostic", "laboratory_testing", "surgery"],
        "vet" => &["veterinar"],
        "beauty" => &["beauty_salon", "nail_salon", "spa", "massage", "hair_removal", "eyelash", "skin_care", "tanning", "personal_or_beauty_service", "wellness_service"],
        "hairdresser" => &["hair_salon", "barber", "hair_stylist"],
        "fitness" => &["gym", "fitness", "yoga", "pilates", "boxing", "martial_arts", "dance_studio"],
        "car_repair" => &["automotive_repair", "auto_repair", "car_repair", "auto_body", "auto_electric", "transmission"],
        "tyres" => &["tire"],
        "car_wash" => &["car_wash", "auto_detailing"],
        "cafe" => &["cafe", "coffee", "tea_room", "dessert", "bakery"],
        "restaurant" => &["restaurant", "steakhouse", "gastropub"],
        "hotel" => &["hotel", "lodging", "hostel", "bed_and_breakfast", "motel", "guest_house"],
        "lawyer" => &["attorney", "law_firm", "legal_service", "lawyer"],
        "notary" => &["notar"],
        "estate_agent" => &["real_estate"],
        "florist" => &["flower", "florist"],
        "electronics_repair" => &["phone_repair", "electronics_repair", "computer_repair", "appliance_repair"],
        "dry_cleaning" => &["dry_clean", "laundr"],
        "optician" => &["eyewear", "optic", "vision_or_eye_care"],
        "language_school" => &["language_school"],
        "driving_school" => &["driving_school", "traffic_school"],
        "furniture" => &["furniture"],
        "hardware" => &["hardware", "building_supply", "home_improvement"],
        _ => &[],
    }
}

/// «Все организации» в Overture: точные `basic_category`, которые не бизнес.
pub const OVERTURE_NOT_BUSINESS: &[&str] = &[
    "park", "lake", "river", "historic_site", "monument", "public_plaza", "public_fountain", "bridge", "government_office",
    "embassy", "police_station", "courthouse", "jail_or_prison", "military_site", "christian_place_of_worship",
    "jewish_place_of_worship", "muslim_place_of_worship", "buddhist_place_of_worship", "hindu_place_of_worship",
    "place_of_worship", "religious_organization", "train_station", "bus_station", "public_transit_facility_or_service",
    "air_transport_facility_or_service", "parking", "atm", "campus_building", "college_university", "elementary_school",
    "library", "community_and_government", "political_organization", "labor_union", "civic_organization",
    "social_or_community_service", "sport_league", "sport_team", "cemetery", "mountain", "beach", "forest", "island",
    "travel_and_transportation", "public_toilet", "town_hall",
];
