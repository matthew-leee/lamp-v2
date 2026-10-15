-- 0005_flagship_media_specs.sql — flagship media slots, architectural specs
-- and the refined editorial pass (Milestone R1).
--
-- What lands here, and why:
--
-- * products gains the flagship media slots (day / lit image URLs) and the
--   architectural spec block (designer, design_year, dimensions,
--   light_source). They live on products, not in product_content, because
--   they are per-SKU FACTS checked at inspection, not locale-keyed copy.
-- * stock_quarantined (STOCK-01): returned or damaged units are quarantined,
--   never silently restocked. The cross-column invariant
--   stock_reserved + stock_quarantined <= stock_total stays application-side
--   (same contract as 0001 for stock_reserved); the column CHECK keeps the
--   count non-negative.
-- * art_variant grows from ('dome', 'shades') to
--   ('dome', 'shades', 'shades-tall', 'piano') so each SKU's placeholder art
--   matches its real silhouette (the PH 3/2 shades tower over the PH 2/1;
--   the PH 1/1 Piano has its own counterweighted form).
-- * product_content heritage/electrical copy and the site_content editorial
--   keys are REPLACED here (0003 was the bootstrap copy): this migration is
--   the single place the refined luxury editorial voice lands for the
--   seeded drop. notFound.title / notFound.description are deliberately NOT
--   touched: they stay exactly as seeded (LANG-01, pinned by tests).
--
-- Idempotence posture matches the seed (0003): the per-SKU UPDATEs and the
-- content upserts are re-runnable and safe, while the ALTER statements run
-- once (a migration is applied once). The content writes use the upsert form
-- (ON CONFLICT DO UPDATE) so a row that an admin already edited is
-- overwritten ON PURPOSE at this one moment: this is the verified flagship
-- copy, applied before the storefront serves it. Later admin edits win
-- because nothing re-runs this file.

-- ---------------------------------------------------------------------------
-- 1. Flagship media slots + architectural specs + quarantine count
-- ---------------------------------------------------------------------------

ALTER TABLE products
    ADD COLUMN IF NOT EXISTS image_day_url TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS image_lit_url TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS designer TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS design_year TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS dimensions TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS light_source TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS stock_quarantined INT NOT NULL DEFAULT 0
        CHECK (stock_quarantined >= 0);

-- ---------------------------------------------------------------------------
-- 2. art_variant: the four verified silhouettes
-- ---------------------------------------------------------------------------

ALTER TABLE products DROP CONSTRAINT IF EXISTS products_art_variant_check;
ALTER TABLE products ADD CONSTRAINT products_art_variant_check
    CHECK (art_variant IN ('dome', 'shades', 'shades-tall', 'piano'));

-- ---------------------------------------------------------------------------
-- 3. The 8 seeded SKUs: verified demo imagery + architectural specs
-- ---------------------------------------------------------------------------

UPDATE products SET
    art_variant = 'dome',
    designer = 'Verner Panton',
    design_year = '1971',
    dimensions = 'Ø 160 × H 238 mm',
    light_source = '2700K Integrated LED · 2.5W · Stepless / 3-Step Dimmer',
    image_day_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/Opal-Beige.png',
    image_lit_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/50422_526bb8d3-8bb4-4182-8b1a-968313788068.webp'
WHERE slug = 'panthella-160-v3-beige';

UPDATE products SET
    art_variant = 'dome',
    designer = 'Verner Panton',
    design_year = '1971',
    dimensions = 'Ø 160 × H 238 mm',
    light_source = '2700K Integrated LED · 2.5W · Stepless / 3-Step Dimmer',
    image_day_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/54067_54067_Panthella-Portable-White-V2-02-1-5-91771.webp',
    image_lit_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/pdp-panthella160portable-gallery-2.webp'
WHERE slug = 'panthella-160-v3-white-opal';

UPDATE products SET
    art_variant = 'dome',
    designer = 'Verner Panton',
    design_year = '1971',
    dimensions = 'Ø 160 × H 238 mm',
    light_source = '2700K Integrated LED · 2.5W · Stepless / 3-Step Dimmer',
    image_day_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/Chrome-Opal-Beige.png',
    image_lit_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/pdp-panthella160portable-gallery-4.webp'
WHERE slug = 'panthella-160-v3-opal-beige-chrome';

UPDATE products SET
    art_variant = 'dome',
    designer = 'Verner Panton',
    design_year = '1971',
    dimensions = 'Ø 160 × H 238 mm',
    light_source = '2700K Integrated LED · 2.5W · Stepless / 3-Step Dimmer',
    image_day_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/60095_60095_Panthella-160-Table-Opal-Pale-Blue.webp',
    image_lit_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/54472_54481_Panthella_20Portable_20PaleBlue-09-2-5-91771.webp'
WHERE slug = 'panthella-160-v3-opal-pale-blue';

UPDATE products SET
    art_variant = 'dome',
    designer = 'Verner Panton',
    design_year = '1971',
    dimensions = 'Ø 160 × H 238 mm',
    light_source = '2700K Integrated LED · 2.5W · Stepless / 3-Step Dimmer',
    image_day_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/60096_60096_Panthella-160-Table-Opal-Pale-Rose.webp',
    image_lit_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/54470_54485_Panthella_20Portable_20PaleRose-08-2-5-91771.webp'
WHERE slug = 'panthella-160-v3-opal-pale-rose';

UPDATE products SET
    art_variant = 'shades',
    designer = 'Poul Henningsen',
    design_year = '1927',
    dimensions = 'Ø 200 × H 355 mm',
    light_source = 'E14 · 2700K Warm White · Hand-Blown Triple Opal Glass',
    image_day_url = 'https://cdn.shopify.com/s/files/1/0913/3370/products/10596_10596_PH-2-1-Table-Chrome-01-1-5-91418.png',
    image_lit_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/50422_83e31b37-2707-4a9c-837f-f1eafcc0218e.webp'
WHERE slug = 'ph-2-1-table-chrome';

UPDATE products SET
    art_variant = 'shades-tall',
    designer = 'Poul Henningsen',
    design_year = '1927',
    dimensions = 'Ø 290 × H 472 mm',
    light_source = 'E14 · 2700K Warm White · Hand-Blown Triple Opal Glass',
    image_day_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/50422_bc1e9700-a5e1-44d8-a59d-e85588b325ae.webp',
    image_lit_url = 'https://cdn.shopify.com/s/files/1/0913/3370/files/50422_ea289ff4-c40b-4d32-bcd1-ffa0f8108e5b.webp'
WHERE slug = 'ph-3-2-table-chrome';

UPDATE products SET
    art_variant = 'piano',
    designer = 'Poul Henningsen',
    design_year = '1931 / 2026 Collector Issue',
    dimensions = 'Ø 140 × H 350 mm',
    light_source = 'Integrated Warm LED · 2700K · Four-Layer Dusty Terracotta Opal Glass',
    image_day_url = 'https://dam.louispoulsen.dk/DigizuiteCore/LegacyService/api/assetstream/68523/50422.webp?width=800&height=800&fit=crop&format=webp&quality=90',
    image_lit_url = 'https://content.louispoulsen.com/globalassets/3.-catalogue-private/table/ph-11-limited-edition---piano-lamp/limited2026-pdp-module3-2.png?maxwidth=1200'
WHERE slug = 'ph-1-1-piano-table-limited';

-- ---------------------------------------------------------------------------
-- 4. product_content: refined heritage / electrical editorial copy
-- ---------------------------------------------------------------------------

-- Family copy repeats across a family's colourways (the 0002 tradeoff), so
-- every SKU of a family carries the same rows as in 0003. Upsert, not plain
-- UPDATE: the rows exist from 0003, but the upsert keeps the file safe to run
-- against a database whose content rows were cleared.
INSERT INTO product_content (product_id, locale, field, value)
SELECT p.id, v.locale, v.field, v.value
FROM (VALUES
    -- heritage: Panthella family (5 SKUs, both locales)
    ('panthella-160-v3-beige', 'en', 'heritage',
     'Conceived by Verner Panton in 1971, the Panthella is an icon of organic Danish modernism where both the hemispherical shade and flared trumpet base act as optical reflectors. In the 160 Portable edition, translucent opal acrylic softens the downward beam into a warm, atmospheric halo built for intimate tables and shelving.'),
    ('panthella-160-v3-white-opal', 'en', 'heritage',
     'Conceived by Verner Panton in 1971, the Panthella is an icon of organic Danish modernism where both the hemispherical shade and flared trumpet base act as optical reflectors. In the 160 Portable edition, translucent opal acrylic softens the downward beam into a warm, atmospheric halo built for intimate tables and shelving.'),
    ('panthella-160-v3-opal-beige-chrome', 'en', 'heritage',
     'Conceived by Verner Panton in 1971, the Panthella is an icon of organic Danish modernism where both the hemispherical shade and flared trumpet base act as optical reflectors. In the 160 Portable edition, translucent opal acrylic softens the downward beam into a warm, atmospheric halo built for intimate tables and shelving.'),
    ('panthella-160-v3-opal-pale-blue', 'en', 'heritage',
     'Conceived by Verner Panton in 1971, the Panthella is an icon of organic Danish modernism where both the hemispherical shade and flared trumpet base act as optical reflectors. In the 160 Portable edition, translucent opal acrylic softens the downward beam into a warm, atmospheric halo built for intimate tables and shelving.'),
    ('panthella-160-v3-opal-pale-rose', 'en', 'heritage',
     'Conceived by Verner Panton in 1971, the Panthella is an icon of organic Danish modernism where both the hemispherical shade and flared trumpet base act as optical reflectors. In the 160 Portable edition, translucent opal acrylic softens the downward beam into a warm, atmospheric halo built for intimate tables and shelving.'),
    ('panthella-160-v3-beige', 'zh-HK', 'heritage',
     '由丹麥設計大師 Verner Panton 於 1971 年創作，Panthella 是有機現代主義的代表作，其半球形燈罩與喇叭形底座同時作為光學反射面。160 便攜版透過半透明蛋白石壓克力柔化向下投射的光束，化為溫暖純淨的光暈，適合置於餐桌、書架或窗台。'),
    ('panthella-160-v3-white-opal', 'zh-HK', 'heritage',
     '由丹麥設計大師 Verner Panton 於 1971 年創作，Panthella 是有機現代主義的代表作，其半球形燈罩與喇叭形底座同時作為光學反射面。160 便攜版透過半透明蛋白石壓克力柔化向下投射的光束，化為溫暖純淨的光暈，適合置於餐桌、書架或窗台。'),
    ('panthella-160-v3-opal-beige-chrome', 'zh-HK', 'heritage',
     '由丹麥設計大師 Verner Panton 於 1971 年創作，Panthella 是有機現代主義的代表作，其半球形燈罩與喇叭形底座同時作為光學反射面。160 便攜版透過半透明蛋白石壓克力柔化向下投射的光束，化為溫暖純淨的光暈，適合置於餐桌、書架或窗台。'),
    ('panthella-160-v3-opal-pale-blue', 'zh-HK', 'heritage',
     '由丹麥設計大師 Verner Panton 於 1971 年創作，Panthella 是有機現代主義的代表作，其半球形燈罩與喇叭形底座同時作為光學反射面。160 便攜版透過半透明蛋白石壓克力柔化向下投射的光束，化為溫暖純淨的光暈，適合置於餐桌、書架或窗台。'),
    ('panthella-160-v3-opal-pale-rose', 'zh-HK', 'heritage',
     '由丹麥設計大師 Verner Panton 於 1971 年創作，Panthella 是有機現代主義的代表作，其半球形燈罩與喇叭形底座同時作為光學反射面。160 便攜版透過半透明蛋白石壓克力柔化向下投射的光束，化為溫暖純淨的光暈，適合置於餐桌、書架或窗台。'),
    -- heritage: PH 2/1 + PH 3/2 (the three-shade table lamps)
    ('ph-2-1-table-chrome', 'en', 'heritage',
     'Poul Henningsen unveiled his logarithmic three-shade system at the 1925 Paris Exposition, solving the central problem of modern electric lighting: eliminating glare without sacrificing warmth. Each shade is mouth-blown from triple-layered opal glass, glossy on the exterior and sandblasted matte beneath to sculpt a calm downward pool of light.'),
    ('ph-3-2-table-chrome', 'en', 'heritage',
     'Poul Henningsen unveiled his logarithmic three-shade system at the 1925 Paris Exposition, solving the central problem of modern electric lighting: eliminating glare without sacrificing warmth. Each shade is mouth-blown from triple-layered opal glass, glossy on the exterior and sandblasted matte beneath to sculpt a calm downward pool of light.'),
    ('ph-2-1-table-chrome', 'zh-HK', 'heritage',
     'Poul Henningsen 於 1925 年巴黎國際裝飾藝術博覽會發表對數螺旋三層燈罩系統，解決了現代電燈的核心難題：在消除眩光的同時保留溫暖光感。每片燈罩均由三層手工吹製蛋白石玻璃打造，外層光亮、內層噴砂霧面，將光線精準導向柔和的下落光池。'),
    ('ph-3-2-table-chrome', 'zh-HK', 'heritage',
     'Poul Henningsen 於 1925 年巴黎國際裝飾藝術博覽會發表對數螺旋三層燈罩系統，解決了現代電燈的核心難題：在消除眩光的同時保留溫暖光感。每片燈罩均由三層手工吹製蛋白石玻璃打造，外層光亮、內層噴砂霧面，將光線精準導向柔和的下落光池。'),
    -- heritage: PH 1/1 Piano (its own story, not the three-shade family line)
    ('ph-1-1-piano-table-limited', 'en', 'heritage',
     'Originally designed by Poul Henningsen in 1931 to illuminate sheet music without dazzling the pianist, the PH 1/1 Piano Table Lamp pairs a counterweighted articulating arm with four-layer mouth-blown dusty terracotta glass shades and aged brass hardware. A rare collector reissue bridging functional precision and sculptural poise.'),
    ('ph-1-1-piano-table-limited', 'zh-HK', 'heritage',
     '源於 Poul Henningsen 於 1931 年為鋼琴家設計的樂譜照明燈，PH 1/1 Piano 限量版檯燈結合球形配重活動燈臂、四層手工吹製赤陶色蛋白石玻璃燈罩及復古黃銅燈架。兼具精密光學結構與雕塑美感，為極具收藏價值的限量復刻之作。'),
    -- electrical: Panthella family (5 SKUs, both locales)
    ('panthella-160-v3-beige', 'en', 'electrical',
     'Cordless rechargeable architecture (USB-C charging cable included; optional wireless Qi charging). Integrated 2700K warm-white LED with three-step touch dimming (10% / 33% / 100%) on the top finial. No mains wall plug required in daily use.'),
    ('panthella-160-v3-white-opal', 'en', 'electrical',
     'Cordless rechargeable architecture (USB-C charging cable included; optional wireless Qi charging). Integrated 2700K warm-white LED with three-step touch dimming (10% / 33% / 100%) on the top finial. No mains wall plug required in daily use.'),
    ('panthella-160-v3-opal-beige-chrome', 'en', 'electrical',
     'Cordless rechargeable architecture (USB-C charging cable included; optional wireless Qi charging). Integrated 2700K warm-white LED with three-step touch dimming (10% / 33% / 100%) on the top finial. No mains wall plug required in daily use.'),
    ('panthella-160-v3-opal-pale-blue', 'en', 'electrical',
     'Cordless rechargeable architecture (USB-C charging cable included; optional wireless Qi charging). Integrated 2700K warm-white LED with three-step touch dimming (10% / 33% / 100%) on the top finial. No mains wall plug required in daily use.'),
    ('panthella-160-v3-opal-pale-rose', 'en', 'electrical',
     'Cordless rechargeable architecture (USB-C charging cable included; optional wireless Qi charging). Integrated 2700K warm-white LED with three-step touch dimming (10% / 33% / 100%) on the top finial. No mains wall plug required in daily use.'),
    ('panthella-160-v3-beige', 'zh-HK', 'electrical',
     '無線可充電結構（隨附 USB-C 充電線，亦支援 Qi 無線充電）。內置 2700K 暖白光 LED，頂部金屬圓鈕設有三段式觸控調光（10% / 33% / 100%），日常擺放無需接駁牆身電源插座。'),
    ('panthella-160-v3-white-opal', 'zh-HK', 'electrical',
     '無線可充電結構（隨附 USB-C 充電線，亦支援 Qi 無線充電）。內置 2700K 暖白光 LED，頂部金屬圓鈕設有三段式觸控調光（10% / 33% / 100%），日常擺放無需接駁牆身電源插座。'),
    ('panthella-160-v3-opal-beige-chrome', 'zh-HK', 'electrical',
     '無線可充電結構（隨附 USB-C 充電線，亦支援 Qi 無線充電）。內置 2700K 暖白光 LED，頂部金屬圓鈕設有三段式觸控調光（10% / 33% / 100%），日常擺放無需接駁牆身電源插座。'),
    ('panthella-160-v3-opal-pale-blue', 'zh-HK', 'electrical',
     '無線可充電結構（隨附 USB-C 充電線，亦支援 Qi 無線充電）。內置 2700K 暖白光 LED，頂部金屬圓鈕設有三段式觸控調光（10% / 33% / 100%），日常擺放無需接駁牆身電源插座。'),
    ('panthella-160-v3-opal-pale-rose', 'zh-HK', 'electrical',
     '無線可充電結構（隨附 USB-C 充電線，亦支援 Qi 無線充電）。內置 2700K 暖白光 LED，頂部金屬圓鈕設有三段式觸控調光（10% / 33% / 100%），日常擺放無需接駁牆身電源插座。'),
    -- electrical: PH family (3 SKUs, both locales)
    ('ph-2-1-table-chrome', 'en', 'electrical',
     '220 to 240V, 50Hz (directly compatible with Hong Kong mains voltage without a step-down transformer). Imported in original European specification and inspected in Hong Kong; HK Type-G plug configuration is confirmed with the collector prior to dispatch.'),
    ('ph-3-2-table-chrome', 'en', 'electrical',
     '220 to 240V, 50Hz (directly compatible with Hong Kong mains voltage without a step-down transformer). Imported in original European specification and inspected in Hong Kong; HK Type-G plug configuration is confirmed with the collector prior to dispatch.'),
    ('ph-1-1-piano-table-limited', 'en', 'electrical',
     '220 to 240V, 50Hz (directly compatible with Hong Kong mains voltage without a step-down transformer). Imported in original European specification and inspected in Hong Kong; HK Type-G plug configuration is confirmed with the collector prior to dispatch.'),
    ('ph-2-1-table-chrome', 'zh-HK', 'electrical',
     '220 至 240V、50Hz（直接對應香港市電電壓，無需變壓器）。原裝歐洲規格平行進口並於香港開箱驗收；出庫前將為藏家確認香港 Type-G 英式三腳插頭配置。'),
    ('ph-3-2-table-chrome', 'zh-HK', 'electrical',
     '220 至 240V、50Hz（直接對應香港市電電壓，無需變壓器）。原裝歐洲規格平行進口並於香港開箱驗收；出庫前將為藏家確認香港 Type-G 英式三腳插頭配置。'),
    ('ph-1-1-piano-table-limited', 'zh-HK', 'electrical',
     '220 至 240V、50Hz（直接對應香港市電電壓，無需變壓器）。原裝歐洲規格平行進口並於香港開箱驗收；出庫前將為藏家確認香港 Type-G 英式三腳插頭配置。')
) AS v(slug, locale, field, value)
JOIN products p ON p.slug = v.slug
ON CONFLICT (product_id, locale, field) DO UPDATE SET value = EXCLUDED.value;

-- ---------------------------------------------------------------------------
-- 5. site_content: the refined editorial voice
-- ---------------------------------------------------------------------------

-- {brand} / {pieces} placeholders stay placeholders: the brand NAME is a
-- scalar config value and the drop size is a COUNT, never seeded prose.
-- notFound.title / notFound.description are deliberately absent here (LANG-01
-- copy pinned by tests, unchanged since 0003).
INSERT INTO site_content (key, locale, value) VALUES
    ('hero.eyebrow', 'en', 'EDITION 01 · COPENHAGEN TO HONG KONG'),
    ('hero.eyebrow', 'zh-HK', '第一回企劃 · 哥本哈根至香港'),
    ('hero.lead', 'en',
     '{pieces} numbered pieces of Danish architectural lighting, held in our Hong Kong vault.'),
    ('hero.lead', 'zh-HK', '{pieces} 件編號丹麥建築美學燈飾，實物全數存倉香港。'),
    ('hero.leadPaired', 'en',
     '{brand} curates authentic European design icons, landed in Hong Kong and priced below local showroom list.'),
    ('hero.leadPaired', 'zh-HK',
     '{brand} 獨立策展歐洲經典設計名作，實物抵港，定價低於本地陳列室。'),
    ('drop.framing', 'en',
     'An opening release of {pieces} individually numbered Louis Poulsen pieces. Sourced directly from European design houses, inspected in Hong Kong, and priced approximately 15% under local showroom retail.'),
    ('drop.framing', 'zh-HK',
     '首發呈現 {pieces} 件獨立編號 Louis Poulsen 經典燈飾。由歐洲設計名門平行進口，於香港開箱驗收，定價較本地陳列室低約 15%。'),
    ('drop.piecesHeading', 'en', 'The Numbered Edition (01 to {pieces})'),
    ('drop.piecesHeading', 'zh-HK', '首發編號典藏（01 至 {pieces}）'),
    ('trust.disclosure', 'en',
     '{brand} operates as an independent parallel-import design house in Hong Kong and is not an authorised Louis Poulsen dealer. Every piece is authentic, serial-checked upon arrival, and backed directly by {brand}. All third-party trademarks belong to their respective owners.'),
    ('trust.disclosure', 'zh-HK',
     '{brand} 為香港獨立平行進口設計選物店，並非 Louis Poulsen 授權經銷商。每件作品均為原裝正貨，抵港後經獨立檢驗，並由 {brand} 直接提供售後保障。所有第三方商標均屬其擁有者。'),
    ('warranty.pending', 'en',
     'Every piece is backed by a direct {brand} house warranty rather than a manufacturer dealer warranty. Detailed coverage terms are being finalised and will be published here prior to launch.'),
    ('warranty.pending', 'zh-HK',
     '每件作品均由 {brand} 直接提供自家售後保養（非原廠授權經銷商保養）。詳細保障條款正在最後釐定，將於正式發售前在此公佈。'),
    ('reference.pending', 'en',
     'Local showroom comparison pricing is currently being verified and will be published once cited with source and date.'),
    ('reference.pending', 'zh-HK',
     '本地陳列室比較定價正在核實中，將於確認來源及查核日期後公佈。'),
    ('enquiry.intro', 'en',
     'Speak with our Hong Kong concierge on WhatsApp for private viewing, plug specification, or piece reservation.'),
    ('enquiry.intro', 'zh-HK',
     '透過 WhatsApp 聯絡香港專員，查詢實物細節、插頭規格或預約留貨。'),
    ('enquiry.messageTemplate', 'en',
     'Hello {brand}, I would like to enquire about {subject}.'),
    ('enquiry.messageTemplate', 'zh-HK', '你好 {brand}，我想查詢關於 {subject} 的詳情。'),
    ('enquiry.dropSubject', 'en', 'Edition 01 (Louis Poulsen Opening Drop)'),
    ('enquiry.dropSubject', 'zh-HK', '第一回企劃（Louis Poulsen 首發典藏）'),
    ('atmosphere.dayLabel', 'en', 'Daylight Gallery'),
    ('atmosphere.dayLabel', 'zh-HK', '日光展廳'),
    ('atmosphere.nightLabel', 'en', 'Nocturne 2700K'),
    ('atmosphere.nightLabel', 'zh-HK', '夜覽 2700K'),
    ('atmosphere.caption', 'en',
     'Switch between Daylight Gallery and Nocturne 2700K to observe how mouth-blown opal glass and spun metal shape glare-free warm illumination.'),
    ('atmosphere.caption', 'zh-HK',
     '切換「日光展廳」與「夜覽 2700K」模式，細賞手工吹製蛋白石玻璃與金屬燈罩如何塑造無眩光的溫暖光影。')
ON CONFLICT (key, locale) DO UPDATE SET value = EXCLUDED.value;
