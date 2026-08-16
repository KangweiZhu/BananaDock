pragma Singleton

import QtQuick

/**
 * Every pixel value lives in this one file.
 *
 * How pixel-accurate replication works here: take a macOS Dock screenshot at a
 * known resolution and scale factor, measure each value, drop the real numbers
 * in below. No other file needs to change.
 *
 * The values below are STARTING ESTIMATES for macOS 26 Tahoe defaults; they
 * have not been calibrated against a reference yet. Each one is tagged:
 * [known] taken from Apple's documented defaults, [measure] needs a reference
 * screenshot.
 */
QtObject {
    id: metrics

    // -- Scaling ---------------------------------------------------------
    // macOS expresses sizes in points; on a Retina display 1pt = 2px. Every
    // value here is in points and gets multiplied by scaleFactor at render
    // time. 1.0 means render at 1x.
    property real scaleFactor: 1.0

    function pt(value) {
        return value * metrics.scaleFactor;
    }

    // -- Icons -----------------------------------------------------------
    property real tileSize: 64        // [known] resting icon edge, Apple default 64pt
    property real largeSize: 128      // [known] magnified peak edge, Apple default 128pt
    property real magnificationRange: 2.5  // [measure] influence radius, in tiles

    property bool magnificationEnabled: true

    // -- Panel -----------------------------------------------------------
    property real panelPaddingH: 8    // [measure] horizontal inner padding
    property real panelPaddingV: 8    // [measure] vertical inner padding
    property real panelRadius: 24     // [measure] Tahoe is noticeably rounder than Big Sur's 16
    property real panelBottomGap: 8   // [measure] gap between panel and screen edge

    // Panel height follows from the icon size and padding; not configured separately.
    readonly property real panelHeight: tileSize + panelPaddingV * 2

    // -- Material (Liquid Glass) -----------------------------------------
    // The actual blur/frost is composited by KWin behind the surface via the
    // blur + contrast protocols. What follows is only the translucent tint and
    // highlight stroke layered on top of it.
    property color panelTint: Qt.rgba(1, 1, 1, 0.18)        // [measure]
    property color panelBorderColor: Qt.rgba(1, 1, 1, 0.28) // [measure] top highlight stroke
    property real panelBorderWidth: 1                       // [measure]

    // -- Running indicator dot -------------------------------------------
    property real dotSize: 4          // [measure]
    property real dotBottomMargin: 4  // [measure] dot centre to panel bottom
    property color dotColor: Qt.rgba(1, 1, 1, 0.85)

    // -- Animation --------------------------------------------------------
    property int magnifyDuration: 90   // [measure] how tightly magnification tracks the cursor
    property int bounceDuration: 620   // [measure] one launch bounce
    property real bounceHeight: 28     // [measure]

    // -- Derived ----------------------------------------------------------
    readonly property real maxScale: magnificationEnabled ? (largeSize / tileSize) : 1.0
    // The surface must be taller than the panel so magnified icons have room
    // to grow upwards.
    readonly property real surfaceHeight: pt(largeSize + panelPaddingV * 2 + panelBottomGap + bounceHeight)
}
