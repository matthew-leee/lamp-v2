-- 0003_seed_opening_drop.sql — idempotent seed of the opening drop.
--
-- DATA-02: seeding the database is the allowed way to load product content;
-- the TS catalogue constants are deleted in the web tier after this lands.
-- Idempotence is DO NOTHING: an existing row (a SKU slug, a piece number, a
-- content key) is never overwritten by a re-run, so admin edits always win
-- over the seed. Another company seeds their own data instead of this drop.
--
-- {brand} placeholders: brand copy is templated; the brand NAME is a scalar
-- config value (env default, settings override) and is never seeded as data.
-- model_alt is seeded only for non-Latin locales; the Latin model name lives
-- on products.model.

INSERT INTO products
    (slug, brand, model, price_cents, stock_total, stock_reserved, family,
     art_variant, art_hue, active)
VALUES
    ('panthella-160-v3-beige', 'Louis Poulsen', 'Panthella 160 V3 Portable',
     161500, 2, 0, 'panthella-160-portable', 'dome', 35, TRUE),
    ('panthella-160-v3-white-opal', 'Louis Poulsen', 'Panthella 160 V3 Portable',
     161500, 5, 0, 'panthella-160-portable', 'dome', 40, TRUE),
    ('panthella-160-v3-opal-beige-chrome', 'Louis Poulsen',
     'Panthella 160 V3 Portable', 238000, 2, 0, 'panthella-160-portable',
     'dome', 30, TRUE),
    ('panthella-160-v3-opal-pale-blue', 'Louis Poulsen',
     'Panthella 160 V3 Portable', 161500, 1, 0, 'panthella-160-portable',
     'dome', 205, TRUE),
    ('panthella-160-v3-opal-pale-rose', 'Louis Poulsen',
     'Panthella 160 V3 Portable', 161500, 1, 0, 'panthella-160-portable',
     'dome', 350, TRUE),
    ('ph-2-1-table-chrome', 'Louis Poulsen', 'PH 2/1 Table',
     714000, 3, 0, 'ph-table', 'shades', 210, TRUE),
    ('ph-3-2-table-chrome', 'Louis Poulsen', 'PH 3/2 Table',
     1037000, 1, 0, 'ph-table', 'shades', 210, TRUE),
    ('ph-1-1-piano-table-limited', 'Louis Poulsen', 'PH 1/1 Piano Table',
     1079500, 1, 0, 'ph-table', 'shades', 45, TRUE)
ON CONFLICT (slug) DO NOTHING;

-- Product content, per (product, locale, field). Family copy (heritage,
-- electrical) repeats across a family's colourways per the 0002 tradeoff.

INSERT INTO product_content (product_id, locale, field, value)
SELECT p.id, v.locale, v.field, v.value
FROM (VALUES
    -- model_alt (zh-HK only; Latin name is products.model)
    ('panthella-160-v3-beige', 'zh-HK', 'model_alt',
     'Panthella 160 V3 便攜檯燈'),
    ('panthella-160-v3-white-opal', 'zh-HK', 'model_alt',
     'Panthella 160 V3 便攜檯燈'),
    ('panthella-160-v3-opal-beige-chrome', 'zh-HK', 'model_alt',
     'Panthella 160 V3 便攜檯燈'),
    ('panthella-160-v3-opal-pale-blue', 'zh-HK', 'model_alt',
     'Panthella 160 V3 便攜檯燈'),
    ('panthella-160-v3-opal-pale-rose', 'zh-HK', 'model_alt',
     'Panthella 160 V3 便攜檯燈'),
    ('ph-2-1-table-chrome', 'zh-HK', 'model_alt', 'PH 2/1 檯燈'),
    ('ph-3-2-table-chrome', 'zh-HK', 'model_alt', 'PH 3/2 檯燈'),
    ('ph-1-1-piano-table-limited', 'zh-HK', 'model_alt', 'PH 1/1 Piano 檯燈'),
    -- colourway (both locales)
    ('panthella-160-v3-beige', 'en', 'colourway', 'Beige'),
    ('panthella-160-v3-beige', 'zh-HK', 'colourway', '米色'),
    ('panthella-160-v3-white-opal', 'en', 'colourway', 'White Opal'),
    ('panthella-160-v3-white-opal', 'zh-HK', 'colourway', '白蛋白石色'),
    ('panthella-160-v3-opal-beige-chrome', 'en', 'colourway',
     'Opal Beige/Chrome'),
    ('panthella-160-v3-opal-beige-chrome', 'zh-HK', 'colourway',
     '蛋白石米色配鍍鉻'),
    ('panthella-160-v3-opal-pale-blue', 'en', 'colourway', 'Opal Pale Blue'),
    ('panthella-160-v3-opal-pale-blue', 'zh-HK', 'colourway', '蛋白石淡藍色'),
    ('panthella-160-v3-opal-pale-rose', 'en', 'colourway', 'Opal Pale Rose'),
    ('panthella-160-v3-opal-pale-rose', 'zh-HK', 'colourway', '蛋白石淡玫瑰色'),
    ('ph-2-1-table-chrome', 'en', 'colourway', 'Chrome'),
    ('ph-2-1-table-chrome', 'zh-HK', 'colourway', '鍍鉻'),
    ('ph-3-2-table-chrome', 'en', 'colourway', 'Chrome'),
    ('ph-3-2-table-chrome', 'zh-HK', 'colourway', '鍍鉻'),
    ('ph-1-1-piano-table-limited', 'en', 'colourway', 'Limited'),
    ('ph-1-1-piano-table-limited', 'zh-HK', 'colourway', '限量版'),
    -- heritage: Panthella family (all 5 SKUs, both locales)
    ('panthella-160-v3-beige', 'en', 'heritage',
     'The Panthella was designed by Verner Panton in 1971. Its trumpet-shaped base and domed shade work together to reflect light softly, leaving the silhouette itself as the only ornament. The Panthella 160 Portable carries that profile into a compact rechargeable table lamp.'),
    ('panthella-160-v3-beige', 'zh-HK', 'heritage',
     'Panthella 由 Verner Panton 於 1971 年設計。喇叭形的彎曲底座與半球形燈罩互相呼應，柔和地反射光線，整道輪廓本身就是唯一的裝飾。Panthella 160 便攜版把這道經典線條濃縮成一部小巧的充電式檯燈。'),
    ('panthella-160-v3-white-opal', 'en', 'heritage',
     'The Panthella was designed by Verner Panton in 1971. Its trumpet-shaped base and domed shade work together to reflect light softly, leaving the silhouette itself as the only ornament. The Panthella 160 Portable carries that profile into a compact rechargeable table lamp.'),
    ('panthella-160-v3-white-opal', 'zh-HK', 'heritage',
     'Panthella 由 Verner Panton 於 1971 年設計。喇叭形的彎曲底座與半球形燈罩互相呼應，柔和地反射光線，整道輪廓本身就是唯一的裝飾。Panthella 160 便攜版把這道經典線條濃縮成一部小巧的充電式檯燈。'),
    ('panthella-160-v3-opal-beige-chrome', 'en', 'heritage',
     'The Panthella was designed by Verner Panton in 1971. Its trumpet-shaped base and domed shade work together to reflect light softly, leaving the silhouette itself as the only ornament. The Panthella 160 Portable carries that profile into a compact rechargeable table lamp.'),
    ('panthella-160-v3-opal-beige-chrome', 'zh-HK', 'heritage',
     'Panthella 由 Verner Panton 於 1971 年設計。喇叭形的彎曲底座與半球形燈罩互相呼應，柔和地反射光線，整道輪廓本身就是唯一的裝飾。Panthella 160 便攜版把這道經典線條濃縮成一部小巧的充電式檯燈。'),
    ('panthella-160-v3-opal-pale-blue', 'en', 'heritage',
     'The Panthella was designed by Verner Panton in 1971. Its trumpet-shaped base and domed shade work together to reflect light softly, leaving the silhouette itself as the only ornament. The Panthella 160 Portable carries that profile into a compact rechargeable table lamp.'),
    ('panthella-160-v3-opal-pale-blue', 'zh-HK', 'heritage',
     'Panthella 由 Verner Panton 於 1971 年設計。喇叭形的彎曲底座與半球形燈罩互相呼應，柔和地反射光線，整道輪廓本身就是唯一的裝飾。Panthella 160 便攜版把這道經典線條濃縮成一部小巧的充電式檯燈。'),
    ('panthella-160-v3-opal-pale-rose', 'en', 'heritage',
     'The Panthella was designed by Verner Panton in 1971. Its trumpet-shaped base and domed shade work together to reflect light softly, leaving the silhouette itself as the only ornament. The Panthella 160 Portable carries that profile into a compact rechargeable table lamp.'),
    ('panthella-160-v3-opal-pale-rose', 'zh-HK', 'heritage',
     'Panthella 由 Verner Panton 於 1971 年設計。喇叭形的彎曲底座與半球形燈罩互相呼應，柔和地反射光線，整道輪廓本身就是唯一的裝飾。Panthella 160 便攜版把這道經典線條濃縮成一部小巧的充電式檯燈。'),
    -- heritage: PH family (all 3 SKUs, both locales)
    ('ph-2-1-table-chrome', 'en', 'heritage',
     'The PH lamp system was designed by Poul Henningsen, whose three-shade design first appeared at the 1925 Paris Exhibition. The shades shape the light as it leaves the source, sending a soft, glare-free glow downwards and outwards. PH table lamps bring that lighting principle into a compact format for the home.'),
    ('ph-2-1-table-chrome', 'zh-HK', 'heritage',
     'PH 燈飾系統由 Poul Henningsen 設計，其三層燈罩結構於 1925 年巴黎國際裝飾藝術博覽會首次亮相。燈罩塑造光線離開光源的路徑，把柔和無眩光的光線送往下方與四周。PH 檯燈把這套照明原理帶入小巧的家居比例。'),
    ('ph-3-2-table-chrome', 'en', 'heritage',
     'The PH lamp system was designed by Poul Henningsen, whose three-shade design first appeared at the 1925 Paris Exhibition. The shades shape the light as it leaves the source, sending a soft, glare-free glow downwards and outwards. PH table lamps bring that lighting principle into a compact format for the home.'),
    ('ph-3-2-table-chrome', 'zh-HK', 'heritage',
     'PH 燈飾系統由 Poul Henningsen 設計，其三層燈罩結構於 1925 年巴黎國際裝飾藝術博覽會首次亮相。燈罩塑造光線離開光源的路徑，把柔和無眩光的光線送往下方與四周。PH 檯燈把這套照明原理帶入小巧的家居比例。'),
    ('ph-1-1-piano-table-limited', 'en', 'heritage',
     'The PH lamp system was designed by Poul Henningsen, whose three-shade design first appeared at the 1925 Paris Exhibition. The shades shape the light as it leaves the source, sending a soft, glare-free glow downwards and outwards. PH table lamps bring that lighting principle into a compact format for the home.'),
    ('ph-1-1-piano-table-limited', 'zh-HK', 'heritage',
     'PH 燈飾系統由 Poul Henningsen 設計，其三層燈罩結構於 1925 年巴黎國際裝飾藝術博覽會首次亮相。燈罩塑造光線離開光源的路徑，把柔和無眩光的光線送往下方與四周。PH 檯燈把這套照明原理帶入小巧的家居比例。'),
    -- electrical: verified facts only (no plug promises, commitment 7 / R2-8).
    -- Panthella family (5 SKUs, both locales)
    ('panthella-160-v3-beige', 'en', 'electrical',
     'The Panthella 160 Portable runs on a rechargeable battery, so no mains plug is in use in daily operation. Hong Kong mains supply is 220-240V at 50Hz with Type G sockets. The charging accessories supplied with each lamp will be confirmed at inspection.'),
    ('panthella-160-v3-beige', 'zh-HK', 'electrical',
     'Panthella 160 便攜版內置可充電電池，日常使用時無需接駁市電插頭。香港市電為 220-240V、50Hz，插座規格為 Type G（英式三腳）。每部燈隨附的充電配件將於實物檢查時確認。'),
    ('panthella-160-v3-white-opal', 'en', 'electrical',
     'The Panthella 160 Portable runs on a rechargeable battery, so no mains plug is in use in daily operation. Hong Kong mains supply is 220-240V at 50Hz with Type G sockets. The charging accessories supplied with each lamp will be confirmed at inspection.'),
    ('panthella-160-v3-white-opal', 'zh-HK', 'electrical',
     'Panthella 160 便攜版內置可充電電池，日常使用時無需接駁市電插頭。香港市電為 220-240V、50Hz，插座規格為 Type G（英式三腳）。每部燈隨附的充電配件將於實物檢查時確認。'),
    ('panthella-160-v3-opal-beige-chrome', 'en', 'electrical',
     'The Panthella 160 Portable runs on a rechargeable battery, so no mains plug is in use in daily operation. Hong Kong mains supply is 220-240V at 50Hz with Type G sockets. The charging accessories supplied with each lamp will be confirmed at inspection.'),
    ('panthella-160-v3-opal-beige-chrome', 'zh-HK', 'electrical',
     'Panthella 160 便攜版內置可充電電池，日常使用時無需接駁市電插頭。香港市電為 220-240V、50Hz，插座規格為 Type G（英式三腳）。每部燈隨附的充電配件將於實物檢查時確認。'),
    ('panthella-160-v3-opal-pale-blue', 'en', 'electrical',
     'The Panthella 160 Portable runs on a rechargeable battery, so no mains plug is in use in daily operation. Hong Kong mains supply is 220-240V at 50Hz with Type G sockets. The charging accessories supplied with each lamp will be confirmed at inspection.'),
    ('panthella-160-v3-opal-pale-blue', 'zh-HK', 'electrical',
     'Panthella 160 便攜版內置可充電電池，日常使用時無需接駁市電插頭。香港市電為 220-240V、50Hz，插座規格為 Type G（英式三腳）。每部燈隨附的充電配件將於實物檢查時確認。'),
    ('panthella-160-v3-opal-pale-rose', 'en', 'electrical',
     'The Panthella 160 Portable runs on a rechargeable battery, so no mains plug is in use in daily operation. Hong Kong mains supply is 220-240V at 50Hz with Type G sockets. The charging accessories supplied with each lamp will be confirmed at inspection.'),
    ('panthella-160-v3-opal-pale-rose', 'zh-HK', 'electrical',
     'Panthella 160 便攜版內置可充電電池，日常使用時無需接駁市電插頭。香港市電為 220-240V、50Hz，插座規格為 Type G（英式三腳）。每部燈隨附的充電配件將於實物檢查時確認。'),
    -- PH family (3 SKUs, both locales): EU plug fact, final config at inspection.
    ('ph-2-1-table-chrome', 'en', 'electrical',
     'The corded PH table lamps are supplied with EU plugs. Hong Kong mains supply is 220-240V at 50Hz with Type G sockets. The final plug configuration for Hong Kong will be confirmed at inspection.'),
    ('ph-2-1-table-chrome', 'zh-HK', 'electrical',
     '有線 PH 檯燈出廠時附歐規插頭。香港市電為 220-240V、50Hz，插座規格為 Type G（英式三腳）。香港使用的最終插頭配置將於實物檢查時確認。'),
    ('ph-3-2-table-chrome', 'en', 'electrical',
     'The corded PH table lamps are supplied with EU plugs. Hong Kong mains supply is 220-240V at 50Hz with Type G sockets. The final plug configuration for Hong Kong will be confirmed at inspection.'),
    ('ph-3-2-table-chrome', 'zh-HK', 'electrical',
     '有線 PH 檯燈出廠時附歐規插頭。香港市電為 220-240V、50Hz，插座規格為 Type G（英式三腳）。香港使用的最終插頭配置將於實物檢查時確認。'),
    ('ph-1-1-piano-table-limited', 'en', 'electrical',
     'The corded PH table lamps are supplied with EU plugs. Hong Kong mains supply is 220-240V at 50Hz with Type G sockets. The final plug configuration for Hong Kong will be confirmed at inspection.'),
    ('ph-1-1-piano-table-limited', 'zh-HK', 'electrical',
     '有線 PH 檯燈出廠時附歐規插頭。香港市電為 220-240V、50Hz，插座規格為 Type G（英式三腳）。香港使用的最終插頭配置將於實物檢查時確認。')
) AS v(slug, locale, field, value)
JOIN products p ON p.slug = v.slug
ON CONFLICT DO NOTHING;

-- The numbered drop (DROP-01): pieces 01-16 in table order.
INSERT INTO pieces (piece_no, product_id)
SELECT v.piece_no, p.id
FROM (VALUES
    (1, 'panthella-160-v3-beige'),
    (2, 'panthella-160-v3-beige'),
    (3, 'panthella-160-v3-white-opal'),
    (4, 'panthella-160-v3-white-opal'),
    (5, 'panthella-160-v3-white-opal'),
    (6, 'panthella-160-v3-white-opal'),
    (7, 'panthella-160-v3-white-opal'),
    (8, 'panthella-160-v3-opal-beige-chrome'),
    (9, 'panthella-160-v3-opal-beige-chrome'),
    (10, 'panthella-160-v3-opal-pale-blue'),
    (11, 'panthella-160-v3-opal-pale-rose'),
    (12, 'ph-2-1-table-chrome'),
    (13, 'ph-2-1-table-chrome'),
    (14, 'ph-2-1-table-chrome'),
    (15, 'ph-3-2-table-chrome'),
    (16, 'ph-1-1-piano-table-limited')
) AS v(piece_no, slug)
JOIN products p ON p.slug = v.slug
ON CONFLICT (piece_no) DO NOTHING;

-- Labelled reference prices (TRUST-02), checked 2026-10-12 at MyConcept Hong
-- Kong. The PH 1/1 Piano Table intentionally gets no row: no verified
-- reference exists, so the UI shows verification pending.
INSERT INTO reference_prices
    (product_id, min_price_cents, max_price_cents, source_name, source_url,
     checked_on)
SELECT p.id, v.min_cents, v.max_cents, v.source_name, v.source_url,
       DATE '2026-10-12'
FROM (VALUES
    ('panthella-160-v3-beige', 190000, 280000, 'MyConcept Hong Kong',
     'https://www.myconcept.com.hk/products/panthella-160-portable-table-lamp'),
    ('panthella-160-v3-white-opal', 190000, 280000, 'MyConcept Hong Kong',
     'https://www.myconcept.com.hk/products/panthella-160-portable-table-lamp'),
    ('panthella-160-v3-opal-beige-chrome', 190000, 280000, 'MyConcept Hong Kong',
     'https://www.myconcept.com.hk/products/panthella-160-portable-table-lamp'),
    ('panthella-160-v3-opal-pale-blue', 190000, 280000, 'MyConcept Hong Kong',
     'https://www.myconcept.com.hk/products/panthella-160-portable-table-lamp'),
    ('panthella-160-v3-opal-pale-rose', 190000, 280000, 'MyConcept Hong Kong',
     'https://www.myconcept.com.hk/products/panthella-160-portable-table-lamp'),
    ('ph-2-1-table-chrome', 840000, 940000, 'MyConcept Hong Kong',
     'https://www.myconcept.com.hk/products/louis-poulsen-ph-21-table'),
    ('ph-3-2-table-chrome', 1220000, 1330000, 'MyConcept Hong Kong',
     'https://www.myconcept.com.hk/products/ph-3-2-table')
) AS v(slug, min_cents, max_cents, source_name, source_url)
JOIN products p ON p.slug = v.slug
ON CONFLICT (product_id) DO NOTHING;

-- Site copy (identity, positioning, policy, meta): the brand-bearing and
-- data-bearing strings. Chrome labels (navigation, stock states, section
-- labels) stay in code. {brand} is replaced at render with the resolved
-- config brand name.
INSERT INTO site_content (key, locale, value) VALUES
    ('hero.eyebrow', 'en', 'The opening drop'),
    ('hero.eyebrow', 'zh-HK', '首發登場'),
    ('hero.lead', 'en',
     'Curated designer homeware. The whole opening drop is physically in Hong Kong.'),
    ('hero.lead', 'zh-HK', '精選設計家居。整批首發燈飾俱在香港。'),
    ('hero.leadPaired', 'en',
     '{brand}, curated designer homeware. The whole opening drop is physically in Hong Kong.'),
    ('hero.leadPaired', 'zh-HK',
     '{brand} 精選設計家居。整批首發燈飾俱在香港。'),
    ('drop.framing', 'en',
     'The opening drop: {pieces} numbered pieces of Louis Poulsen lighting.'),
    ('drop.framing', 'zh-HK',
     '首發登場：{pieces} 件編號 Louis Poulsen 燈飾。'),
    -- Drop section heading (company copy: category-bearing, not chrome).
    ('drop.piecesHeading', 'en', 'The pieces in the drop'),
    ('drop.piecesHeading', 'zh-HK', '首發燈飾系列'),
    ('trust.disclosure', 'en',
     '{brand} is an independent seller and is not an authorised Louis Poulsen dealer. All trademarks belong to their owners.'),
    ('trust.disclosure', 'zh-HK',
     '{brand} 為獨立賣家，並非 Louis Poulsen 授權經銷商。所有商標均屬其擁有者。'),
    ('warranty.pending', 'en',
     '{brand} gives its own warranty on every piece it sells. The exact terms are still being finalised and will be published here before the first sale, in {brand} wording. The Louis Poulsen manufacturer warranty is never quoted as the {brand} warranty.'),
    ('warranty.pending', 'zh-HK',
     '{brand} 為所售出的每件燈飾提供自家保養。具體條款仍在釐定中，將以 {brand} 自己的用詞於首宗銷售前在此公佈。Louis Poulsen 的原廠保養絕不會被引用為 {brand} 的保養條款。'),
    ('reference.pending', 'en',
     'Reference price: verification pending. A comparison price is published here only once a real source has been checked and the check date recorded.'),
    ('reference.pending', 'zh-HK',
     '參考價：核實中。只有在核實真實來源並記錄查核日期後，比較價才會在此公佈。'),
    ('enquiry.intro', 'en', 'Questions about a piece? Ask on WhatsApp.'),
    ('enquiry.intro', 'zh-HK', '對某件燈飾有疑問？WhatsApp 查詢。'),
    ('enquiry.messageTemplate', 'en',
     'Hi {brand}, I would like to ask about {subject}.'),
    ('enquiry.messageTemplate', 'zh-HK', '你好 {brand}，我想查詢 {subject}。'),
    ('enquiry.dropSubject', 'en',
     'the opening drop of Louis Poulsen lighting'),
    ('enquiry.dropSubject', 'zh-HK', 'Louis Poulsen 首發燈飾系列'),
    ('meta.home.title', 'en',
     '{brand} | Curated designer homeware, the opening drop'),
    ('meta.home.title', 'zh-HK', '{brand} | 精選設計家居 · 首發登場'),
    ('meta.home.description', 'en',
     '{brand}, curated designer homeware. The opening drop of designer lighting: {pieces} numbered pieces, the whole drop physically in Hong Kong.'),
    ('meta.home.description', 'zh-HK',
     '{brand} 精選設計家居。設計燈飾首發登場，{pieces} 件編號作品，整批俱在香港。'),
    ('meta.product.descriptionTemplate', 'en',
     '{subject}. A numbered piece of the {brand} opening drop of Louis Poulsen lighting. {brand} is an independent seller.'),
    ('meta.product.descriptionTemplate', 'zh-HK',
     '{subject}，{brand} 首發登場的編號 Louis Poulsen 燈飾。{brand} 為獨立賣家。'),
    -- notFound.title/description are SINGLE-LANGUAGE per locale (founder
    -- decision 2026-10-14: one language per page — the old bilingual
    -- metadata strings showed both languages in the browser tab).
    ('notFound.title', 'en', '{brand} | Page not found'),
    ('notFound.title', 'zh-HK', '{brand} | 找不到頁面'),
    ('notFound.description', 'en',
     'Page not found. {brand}, curated designer homeware, Hong Kong.'),
    ('notFound.description', 'zh-HK',
     '找不到頁面。{brand}，香港精選設計家居。'),
    ('notFound.body', 'en',
     'This page does not exist. The two language editions of {brand} are linked below.'),
    ('notFound.body', 'zh-HK',
     '此頁面不存在。以下連結前往 {brand} 兩個語言版本。'),
    -- Footer location line (company location, not a code literal).
    ('footer.location', 'en', 'Hong Kong'),
    ('footer.location', 'zh-HK', '香港')
ON CONFLICT (key, locale) DO NOTHING;

-- settings: intentionally empty. Overrides only (CONFIG-01); env is the
-- default layer and never seeded here. {pieces} placeholders render with
-- the catalogue-derived COUNT (total_pieces), never a prose constant.
-- audit_log: intentionally empty. Written by the D2 admin console.