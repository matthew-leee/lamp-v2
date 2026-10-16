"use client";

import { useState, type CSSProperties } from "react";
import type { ArtVariant } from "@/lib/api";

interface LampStageProps {
  hue: number;
  variant: ArtVariant;
  lit: boolean;
  imageDayUrl?: string;
  imageLitUrl?: string;
  badgeText?: string;
  dimensionsText?: string;
  mode?: "photo" | "scale";
  aspectClass?: string;
  altText?: string;
  compact?: boolean;
}

function ArchitecturalSilhouette({
  variant,
  lit,
  blueprint,
  dimensionsText,
}: {
  variant: ArtVariant;
  lit: boolean;
  blueprint: boolean;
  dimensionsText?: string;
}) {
  const strokeColor = "var(--text-primary)";
  const shadeFill = blueprint
    ? "none"
    : lit
      ? "hsl(var(--art-hue, 38) 72% 82%)"
      : "var(--color-sand)";
  const bodyFill = blueprint ? "none" : "var(--color-linen)";

  return (
    <svg
      viewBox="0 0 480 480"
      className="
        pointer-events-none absolute inset-0 h-full w-full transition-opacity duration-500
      "
      aria-hidden="true"
    >
      {blueprint && (
        <g opacity="0.22" stroke={strokeColor} strokeWidth="0.75">
          <line x1="60" y1="380" x2="420" y2="380" strokeDasharray="4 4" />
          <line x1="88" y1="110" x2="88" y2="380" strokeDasharray="3 3" />
          <line x1="82" y1="110" x2="94" y2="110" />
          <line x1="82" y1="380" x2="94" y2="380" />
          <line x1="140" y1="406" x2="340" y2="406" strokeDasharray="3 3" />
          <rect
            x="355"
            y="245"
            width="28"
            height="135"
            fill="none"
            stroke={strokeColor}
            strokeDasharray="2 2"
          />
          <text
            x="369"
            y="236"
            textAnchor="middle"
            fontSize="9"
            fill={strokeColor}
            fontFamily="monospace"
          >
            220mm Book
          </text>
          {dimensionsText && (
            <text
              x="240"
              y="436"
              textAnchor="middle"
              fontSize="12"
              letterSpacing="2"
              fill={strokeColor}
              fontFamily="monospace"
            >
              {dimensionsText}
            </text>
          )}
        </g>
      )}

      <g
        stroke={blueprint ? strokeColor : "var(--border-hairline)"}
        strokeWidth={blueprint ? "1.6" : "1"}
      >
        {variant === "dome" ? (
          <g>
            <path
              d="M 225 245 C 225 304 210 330 202 372 L 278 372 C 270 330 255 304 255 245 Z"
              fill={bodyFill}
            />
            <ellipse cx="240" cy="374" rx="86" ry="11" fill={bodyFill} />
            <path
              d="M 136 250 A 104 104 0 0 1 344 250 Z"
              fill={shadeFill}
              className="transition-colors duration-700"
            />
            <circle cx="240" cy="142" r="5" fill={bodyFill} />
          </g>
        ) : variant === "shades" ? (
          <g>
            <rect x="232" y="254" width="16" height="114" fill={bodyFill} />
            <ellipse cx="240" cy="372" rx="84" ry="11" fill={bodyFill} />
            <path d="M 126 256 A 114 52 0 0 1 354 256 Z" fill={shadeFill} />
            <path d="M 156 214 A 84 42 0 0 1 324 214 Z" fill={shadeFill} />
            <path d="M 188 176 A 52 32 0 0 1 292 176 Z" fill={shadeFill} />
          </g>
        ) : variant === "shades-tall" ? (
          <g>
            <rect x="232" y="220" width="16" height="150" fill={bodyFill} />
            <ellipse cx="240" cy="374" rx="92" ry="12" fill={bodyFill} />
            <path d="M 112 226 A 128 56 0 0 1 368 226 Z" fill={shadeFill} />
            <path d="M 148 178 A 92 46 0 0 1 332 178 Z" fill={shadeFill} />
            <path d="M 184 134 A 56 34 0 0 1 296 134 Z" fill={shadeFill} />
          </g>
        ) : (
          <g>
            <ellipse cx="236" cy="374" rx="82" ry="11" fill={bodyFill} />
            <rect x="229" y="246" width="14" height="128" fill={bodyFill} />
            <circle cx="148" cy="238" r="24" fill={bodyFill} />
            <path d="M 148 232 L 334 190 L 338 204 L 152 246 Z" fill={bodyFill} />
            <circle cx="236" cy="240" r="9" fill={bodyFill} />
            <path d="M 296 196 A 48 32 0 0 1 392 196 Z" fill={shadeFill} />
          </g>
        )}
      </g>
    </svg>
  );
}

export default function LampStage({
  hue,
  variant,
  lit,
  imageDayUrl = "",
  imageLitUrl = "",
  badgeText,
  dimensionsText,
  mode = "photo",
  aspectClass = "aspect-[4/5]",
  altText,
  compact = false,
}: LampStageProps) {
  const [dayImgFailed, setDayImgFailed] = useState(false);
  const [litImgFailed, setLitImgFailed] = useState(false);

  const validDayUrl =
    !dayImgFailed && imageDayUrl.trim() !== "" ? imageDayUrl.trim() : "";
  const validLitUrl =
    !litImgFailed && imageLitUrl.trim() !== "" ? imageLitUrl.trim() : "";

  // MEDIA-01: when both day & lit photos are configured, toggling 5500K <-> 2700K
  // renders the lit effect (cross-fade when URLs differ, plus 2700K radial bloom).
  // When only a single photo is uploaded (image_day_url only, image_lit_url blank),
  // render it as a clean plain image without artificial warm tinting or bloom.
  const hasLitEffect = validDayUrl !== "" && validLitUrl !== "";
  const primaryDay = validDayUrl || validLitUrl;
  const primaryLit = validLitUrl || validDayUrl;
  const showPhotos = mode === "photo" && primaryDay !== "";
  const canCrossFade =
    hasLitEffect && primaryDay !== "" && primaryLit !== "" && primaryDay !== primaryLit;

  return (
    <div
      className={`stage-surface relative w-full overflow-hidden border border-[var(--border-hairline)] ${aspectClass}`}
      style={{ "--art-hue": String(hue) } as CSSProperties}
      role={altText ? "img" : undefined}
      aria-label={altText}
    >
      {/* Plinth floor line */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 bottom-0 h-[22%] border-t border-[var(--border-hairline)] opacity-50"
      />

      {/* 2700K radial light bloom when lit (only for dual-photo / silhouette items) */}
      <div
        aria-hidden="true"
        className={`halo-2700k pointer-events-none absolute inset-0 ${
          lit && (!showPhotos || hasLitEffect) ? "opacity-100" : "opacity-0"
        }`}
      />

      {showPhotos ? (
        <>
          {/* Daylight sculptural cutout / plain photograph */}
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            src={primaryDay}
            alt=""
            loading="eager"
            decoding="sync"
            onError={() => setDayImgFailed(true)}
            className={`absolute inset-0 h-full w-full object-contain ${
              compact ? "p-2" : "p-7"
            } transition-all duration-700 ease-out group-hover:scale-[1.03] ${
              canCrossFade && lit ? "opacity-0" : "opacity-100"
            }`}
          />
          {canCrossFade && (
            <>
              {/* Nocturne 2700K lit photograph */}
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img
                src={primaryLit}
                alt=""
                loading="eager"
                decoding="sync"
                onError={() => setLitImgFailed(true)}
                className={`absolute inset-0 h-full w-full object-contain ${
                  compact ? "p-2" : "p-7"
                } transition-all duration-700 ease-out group-hover:scale-[1.03] ${
                  lit ? "opacity-100" : "opacity-0"
                }`}
              />
            </>
          )}
          {hasLitEffect && (
            <div
              aria-hidden="true"
              className={`warm-photo-cast pointer-events-none absolute inset-0 ${
                lit ? "opacity-100" : "opacity-0"
              }`}
            />
          )}
        </>
      ) : variant !== "none" ? (
        <ArchitecturalSilhouette
          variant={variant}
          lit={lit}
          blueprint={mode === "scale"}
          dimensionsText={dimensionsText}
        />
      ) : null}

      {/* Top-left numbered edition badge */}
      {badgeText && (
        <div className="pointer-events-none absolute left-3.5 top-3.5 z-10 border border-[var(--border-hairline)] bg-[var(--surface-glass)] px-2.5 py-1 text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)] backdrop-blur-md">
          {badgeText}
        </div>
      )}

      {/* Top-right live Kelvin state indicator */}
      {!compact && (!showPhotos || hasLitEffect) && variant !== "none" && (
        <div className="pointer-events-none absolute right-3.5 top-3.5 z-10 flex items-center gap-1.5 px-2 py-1 text-[10px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
          <span
            className={`inline-block size-2 rounded-full transition-all duration-500 ${
              lit
                ? "bg-[var(--color-glow)] shadow-[0_0_8px_var(--color-glow)]"
                : "border border-[var(--text-secondary)] bg-transparent"
            }`}
          />
          <span>{mode === "scale" ? "1:10 CAD" : lit ? "2700K" : "5500K"}</span>
        </div>
      )}
    </div>
  );
}
