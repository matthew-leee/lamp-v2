"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { usePathname } from "next/navigation";
import type { CatalogueProduct } from "@/lib/api";

export interface StockInfo {
  available: number;
  total: number;
  reserved: number;
  quarantined: number;
}

export interface BagItem {
  slug: string;
  quantity: number;
}

interface StorefrontContextValue {
  lit: boolean;
  setLit: (v: boolean) => void;
  toggleLit: () => void;
  stockMap: Record<string, StockInfo> | null;
  stockLoading: boolean;
  refreshStock: () => Promise<void>;
  bag: BagItem[];
  bagOpen: boolean;
  setBagOpen: (open: boolean) => void;
  addToBag: (slug: string, quantity?: number, openDrawer?: boolean) => void;
  updateBagQuantity: (slug: string, quantity: number) => void;
  removeFromBag: (slug: string) => void;
  clearBag: () => void;
  catalogueProducts: CatalogueProduct[];
  currency: string;
  brandName: string;
}

const StorefrontContext = createContext<StorefrontContextValue | null>(null);

const ATMOSPHERE_STORAGE_KEY = "lamp_atmosphere_lit_v2";
const BAG_STORAGE_KEY = "lamp_collector_bag_v2";

export function StorefrontProvider({
  children,
  catalogueProducts,
  currency,
  brandName,
}: {
  children: ReactNode;
  catalogueProducts: CatalogueProduct[];
  currency: string;
  brandName: string;
}) {
  const [lit, setLitState] = useState<boolean>(true);
  const [stockMap, setStockMap] = useState<Record<string, StockInfo> | null>(null);
  const [stockLoading, setStockLoading] = useState<boolean>(true);
  const [bag, setBag] = useState<BagItem[]>([]);
  const [bagOpen, setBagOpen] = useState<boolean>(false);
  const pathname = usePathname();

  // Restore persisted atmosphere & bag on mount
  useEffect(() => {
    try {
      const savedLit = window.localStorage.getItem(ATMOSPHERE_STORAGE_KEY);
      if (savedLit === "day") {
        setLitState(false);
        document.documentElement.dataset.atmosphere = "day";
      } else {
        setLitState(true);
        document.documentElement.dataset.atmosphere = "night";
      }
      const savedBag = window.localStorage.getItem(BAG_STORAGE_KEY);
      if (savedBag) {
        const parsed = JSON.parse(savedBag) as BagItem[];
        if (Array.isArray(parsed)) {
          setBag(
            parsed.filter(
              (item) =>
                typeof item?.slug === "string" &&
                typeof item?.quantity === "number" &&
                item.quantity >= 1,
            ),
          );
        }
      }
    } catch {
      document.documentElement.dataset.atmosphere = "night";
    }
  }, []);

  const setLit = useCallback((next: boolean) => {
    setLitState(next);
    try {
      document.documentElement.dataset.atmosphere = next ? "night" : "day";
      window.localStorage.setItem(ATMOSPHERE_STORAGE_KEY, next ? "night" : "day");
    } catch {
      // ignore storage errors
    }
  }, []);

  const toggleLit = useCallback(() => {
    setLitState((prev) => {
      const next = !prev;
      try {
        document.documentElement.dataset.atmosphere = next ? "night" : "day";
        window.localStorage.setItem(ATMOSPHERE_STORAGE_KEY, next ? "night" : "day");
      } catch {
        // ignore
      }
      return next;
    });
  }, []);

  // Keyboard shortcut 'L' toggles room lighting when not typing in an input
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.tagName === "SELECT" ||
          target.isContentEditable)
      ) {
        return;
      }
      if (e.key === "l" || e.key === "L") {
        e.preventDefault();
        toggleLit();
      } else if (e.key === "Escape" && bagOpen) {
        setBagOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [toggleLit, bagOpen]);

  const refreshStock = useCallback(async () => {
    try {
      const res = await fetch("/api/availability", { cache: "no-store" });
      if (!res.ok) {
        setStockMap(null);
        return;
      }
      const data = (await res.json()) as {
        ok?: boolean;
        stock?: Record<string, StockInfo>;
      };
      if (data.ok && data.stock) {
        setStockMap(data.stock);
      }
    } catch {
      setStockMap(null);
    } finally {
      setStockLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshStock();
  }, [refreshStock, pathname]);

  const persistBag = useCallback((nextBag: BagItem[]) => {
    setBag(nextBag);
    try {
      window.localStorage.setItem(BAG_STORAGE_KEY, JSON.stringify(nextBag));
    } catch {
      // ignore
    }
  }, []);

  const addToBag = useCallback(
    (slug: string, quantity = 1, openDrawer = true) => {
      setBag((prev) => {
        const maxAvail = stockMap?.[slug]?.available ?? 16;
        if (maxAvail <= 0) return prev;
        const existing = prev.find((i) => i.slug === slug);
        let next: BagItem[];
        if (existing) {
          const clamped = Math.min(maxAvail, existing.quantity + quantity);
          next = prev.map((i) => (i.slug === slug ? { ...i, quantity: clamped } : i));
        } else {
          next = [...prev, { slug, quantity: Math.min(maxAvail, quantity) }];
        }
        try {
          window.localStorage.setItem(BAG_STORAGE_KEY, JSON.stringify(next));
        } catch {
          // ignore
        }
        return next;
      });
      if (openDrawer) {
        setBagOpen(true);
      }
    },
    [stockMap],
  );

  const updateBagQuantity = useCallback(
    (slug: string, quantity: number) => {
      if (quantity <= 0) {
        persistBag(bag.filter((i) => i.slug !== slug));
        return;
      }
      const maxAvail = stockMap?.[slug]?.available ?? 16;
      const clamped = Math.min(Math.max(1, quantity), Math.max(1, maxAvail));
      persistBag(bag.map((i) => (i.slug === slug ? { ...i, quantity: clamped } : i)));
    },
    [bag, persistBag, stockMap],
  );

  const removeFromBag = useCallback(
    (slug: string) => {
      persistBag(bag.filter((i) => i.slug !== slug));
    },
    [bag, persistBag],
  );

  const clearBag = useCallback(() => {
    persistBag([]);
  }, [persistBag]);

  return (
    <StorefrontContext.Provider
      value={{
        lit,
        setLit,
        toggleLit,
        stockMap,
        stockLoading,
        refreshStock,
        bag,
        bagOpen,
        setBagOpen,
        addToBag,
        updateBagQuantity,
        removeFromBag,
        clearBag,
        catalogueProducts,
        currency,
        brandName,
      }}
    >
      {children}
    </StorefrontContext.Provider>
  );
}

export function useStorefront(): StorefrontContextValue {
  const ctx = useContext(StorefrontContext);
  if (!ctx) {
    throw new Error("useStorefront must be used inside <StorefrontProvider>");
  }
  return ctx;
}
