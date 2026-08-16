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
    // tileSize is the layout PITCH -- the centre-to-centre spacing of icons --
    // not the size of the icon artwork. The reference shows those differ.
    property real tileSize: 64        // [known] Apple default tile pitch, 64pt
    property real largeSize: 128      // [known] magnified peak pitch, Apple default 128pt

    // [derived] Icon artwork is smaller than its tile, leaving gaps between
    // icons. Ratio measured off the WWDC25 reference: iconArt/pitch = 45/67.
    property real iconSizeRatio: 0.67
    readonly property real iconSize: tileSize * iconSizeRatio
    property real magnificationRange: 2.5  // [measure] influence radius, in tiles

    property bool magnificationEnabled: true

    // -- Auto-hide ---------------------------------------------------------
    property bool autoHide: true
    property int autoHideDelay: 500   // [measure] ms the pointer must be away

    // -- Panel -----------------------------------------------------------
    property real panelPaddingH: 8    // [measure] horizontal inner padding
    property real panelBottomGap: 8   // [measure] gap between panel and screen edge

    // [derived] Panel height relative to the tile pitch, measured off the
    // reference as 89/67. With the 64pt default pitch this gives an 85pt panel.
    property real panelHeightRatio: 1.33
    readonly property real panelHeight: tileSize * panelHeightRatio

    // [derived] Icons sit slightly above centre; the extra room underneath is
    // where the running dots go. Reference: 20px above, 24px below, of 89px.
    property real iconTopPadRatio: 0.225
    readonly property real iconBottomMargin: panelHeight * (1.0 - iconTopPadRatio) - iconSize

    // [reference] Tahoe's dock is a capsule -- the end caps are semicircular, so
    // the radius is exactly half the height. Measured off Apple's WWDC25 press
    // shot; this replaced an earlier guess of a 24pt rounded rectangle.
    property real panelRadiusRatio: 0.5
    readonly property real panelRadius: panelHeight * panelRadiusRatio

    // -- Material (Liquid Glass) -----------------------------------------
    // The actual blur/frost is composited by KWin behind the surface via the
    // blur + contrast protocols. What follows is only the translucent tint and
    // highlight stroke layered on top of it.
    // [reference] The glass is far more transparent than first assumed -- in the
    // reference the wallpaper's colour reads clearly through the panel. Most of
    // the look comes from the compositor's blur, not from this tint, so keep it
    // light or the blur gets washed out.
    property color panelTint: Qt.rgba(1, 1, 1, 0.10)
    // [reference] A bright hairline runs along both the top and bottom edges.
    property color panelBorderColor: Qt.rgba(1, 1, 1, 0.30)
    property real panelBorderWidth: 1                       // [measure]

    // -- Running indicator dot -------------------------------------------
    property real dotSize: 4          // [measure]
    property real dotBottomMargin: 6  // [measure] dot centre to panel bottom
    property color dotColor: Qt.rgba(1, 1, 1, 0.85)

    // -- Animation --------------------------------------------------------
    property int magnifyDuration: 90   // [measure] how tightly magnification tracks the cursor
    property int bounceDuration: 620   // [measure] one launch bounce
    property real bounceHeight: 28     // [measure]

    // -- Derived ----------------------------------------------------------
    readonly property real maxScale: magnificationEnabled ? (largeSize / tileSize) : 1.0
    // The surface must be taller than the panel so magnified icons have room
    // to grow upwards.
    /// Icon artwork at full magnification.
    readonly property real maxIconSize: largeSize * iconSizeRatio

    // The surface has to contain the panel, the gap below it, and the headroom a
    // fully magnified icon needs as it grows upward past the panel's top edge.
    readonly property real surfaceHeight: pt(panelBottomGap
                                             + Math.max(panelHeight,
                                                        iconBottomMargin + maxIconSize)
                                             + bounceHeight)
}
