export type Locale = "en" | "zh-HK";

export const locales: readonly Locale[] = ["en", "zh-HK"] as const;
export const defaultLocale: Locale = "en";

export const htmlLang: Record<Locale, string> = {
  en: "en",
  "zh-HK": "zh-Hant-HK",
};

export const languageEndonyms: Record<Locale, string> = {
  en: "EN",
  "zh-HK": "繁體中文",
};

export function isLocale(value: string): value is Locale {
  return value === "en" || value === "zh-HK";
}

export function isCjk(locale: Locale): boolean {
  return locale === "zh-HK";
}

export type BilingualText = Record<Locale, string>;

export function t(map: BilingualText, locale: Locale): string {
  return map[locale] ?? map.en;
}

export const ui = {
  navCollection: { en: "Collection", "zh-HK": "典藏系列" },
  navEditionMatrix: { en: "Edition 01–16", "zh-HK": "編號 01–16" },
  navProvenance: { en: "Provenance", "zh-HK": "平行進口來源" },
  navOrderLookup: { en: "Order Lookup", "zh-HK": "訂單查詢" },
  navBag: { en: "Bag", "zh-HK": "購物袋" },
  atmosphereDay: { en: "Daylight 5500K", "zh-HK": "日光 5500K" },
  atmosphereNight: { en: "Nocturne 2700K", "zh-HK": "夜覽 2700K" },
  viewCanvas: { en: "Exhibition Grid", "zh-HK": "展廳陳列" },
  viewIndex: { en: "Archival Index", "zh-HK": "典藏目錄" },
  filterAll: { en: "All Pieces", "zh-HK": "全部作品" },
  openEdition: { en: "Open Edition", "zh-HK": "常設單品" },
  stockAvailable: (n: number) => ({
    en: `${n} available in HK vault`,
    "zh-HK": `香港存倉尚餘 ${n} 件`,
  }),
  stockSoldOut: { en: "Edition Collected", "zh-HK": "典藏已售罄" },
  stockLoading: { en: "Checking vault...", "zh-HK": "核實存倉中..." },
  addToBag: { en: "Add to Bag", "zh-HK": "加入購物袋" },
  addedToBag: { en: "Added to Bag", "zh-HK": "已加入購物袋" },
  buyNow: { en: "Buy Now", "zh-HK": "立即選購" },
  viewDetails: { en: "Inspect Piece", "zh-HK": "細賞作品" },
  backToCollection: { en: "Back to Exhibition", "zh-HK": "返回首發典藏" },
  stageModeDay: { en: "Daylight Cutout", "zh-HK": "日光實物" },
  stageModeNight: { en: "2700K Lit", "zh-HK": "2700K 點亮" },
  stageModeScale: { en: "Scale & Blueprint", "zh-HK": "比例與藍圖" },
  colourwaysHeading: { en: "Finish & Colourway", "zh-HK": "表面處理與配色" },
  marketPriceTitle: { en: "Market Price Transparency", "zh-HK": "市場透明定價比較" },
  housePriceLabel: { en: "House Direct Price", "zh-HK": "本店直購價" },
  dealerRangeLabel: { en: "HK Authorised Showroom", "zh-HK": "香港授權陳列室定價" },
  verifiedOnLabel: { en: "Verified on", "zh-HK": "查核日期" },
  specDesigner: { en: "Designer", "zh-HK": "設計師" },
  specYear: { en: "Year", "zh-HK": "設計年份" },
  specDimensions: { en: "Dimensions", "zh-HK": "尺寸規格" },
  specLightSource: { en: "Light Source", "zh-HK": "光源規格" },
  specElectrical: { en: "HK Electrical & Plug", "zh-HK": "香港電壓與插頭規格" },
  specWarranty: { en: "House Warranty", "zh-HK": "自家售後保養" },
  specDisclosure: { en: "Parallel-Import Provenance", "zh-HK": "平行進口來源聲明" },
  specHeritage: { en: "Architectural Heritage", "zh-HK": "建築與光學傳承" },
  conciergeCta: { en: "WhatsApp Private Concierge", "zh-HK": "WhatsApp 私人專員查詢" },
  deliveryNote: {
    en: "Complimentary Hong Kong courier or personal hand-delivery arranged upon settlement.",
    "zh-HK": "完成付款後，由專人安排香港免費速遞或親自送遞交收。",
  },
  bagTitle: { en: "Collector's Bag", "zh-HK": "藏家購物袋" },
  bagEmptyTitle: { en: "Your collector's bag is currently empty.", "zh-HK": "你的藏家購物袋目前沒有作品。" },
  bagEmptySubtitle: {
    en: "Explore highlighted pieces from the opening drop below:",
    "zh-HK": "瀏覽以下首發企劃精選編號作品：",
  },
  bagSubtotal: { en: "Subtotal", "zh-HK": "小計" },
  bagProceedCheckout: { en: "Proceed to Checkout & Settlement", "zh-HK": "前往結帳及付款" },
  bagClear: { en: "Clear Bag", "zh-HK": "清空購物袋" },
  checkoutTitle: { en: "Checkout & Payment Settlement", "zh-HK": "結帳及付款交收" },
  orderStatusMap: {
    reserved: { en: "Reserved · Awaiting Payment", "zh-HK": "已留貨 · 待完成付款" },
    paid: { en: "Paid · Confirmed", "zh-HK": "已付款 · 訂單已確認" },
    fulfilled: { en: "Fulfilled · Dispatched", "zh-HK": "已完成 · 已安排派遞" },
    expired: { en: "Hold Expired", "zh-HK": "留貨時限已過" },
    cancelled: { en: "Cancelled", "zh-HK": "已取消" },
    late_payment_review: { en: "Payment Under Concierge Review", "zh-HK": "款項正由專員核實中" },
    returned_quarantined: { en: "Returned · In Inspection", "zh-HK": "已退回 · 檢驗中" },
  } as Record<string, BilingualText>,
};
