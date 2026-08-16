import QtQuick

import MacDock

/**
 * The layer-shell surface.
 *
 * It spans the whole bottom edge of the screen and is much taller than the
 * panel -- the extra height is headroom for magnified icons to grow into. The
 * surplus transparent area is excluded via the input region so it does not
 * swallow clicks.
 */
Window {
    id: window

    // Must start hidden: the layer-shell role can only be set while the surface
    // is created, so C++ calls attach() first and show() after.
    visible: false
    color: "transparent"

    width: Screen.width
    height: Metrics.surfaceHeight

    DockPanel {
        id: panel

        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Metrics.pt(Metrics.panelBottomGap)

        // The resting layout's origin is fixed -- it does not move as icons
        // magnify -- so the pointer can be mapped straight into it without
        // creating a deformation feedback loop.
        readonly property real restOriginX:
            (window.width - panel.tilePx * Math.max(panel.itemWidths.length, 1)) / 2

        HoverHandler {
            id: hover

            onPointChanged: {
                panel.cursorRestX = hover.point.position.x
                                    + panel.x
                                    - panel.restOriginX;
            }
        }
    }

    // Collapse the magnification once the pointer leaves the panel, and start
    // the auto-hide countdown.
    HoverHandler {
        id: windowHover
        onHoveredChanged: {
            if (windowHover.hovered) {
                hideTimer.stop();
                if (Metrics.autoHide) {
                    DockSurface.setHidden(false);
                }
            } else {
                panel.cursorRestX = -1;
                if (Metrics.autoHide) {
                    hideTimer.restart();
                }
            }
        }
    }

    Timer {
        id: hideTimer
        interval: Metrics.autoHideDelay
        onTriggered: DockSurface.setHidden(true)
    }

    // -- Keeping the compositor in sync ------------------------------------
    function syncSurface() {
        // Windows only need to avoid the panel at rest, not the magnification
        // headroom above it -- same as macOS. An auto-hiding dock reserves
        // nothing, so windows get the full screen.
        DockSurface.setExclusiveZone(Metrics.autoHide
            ? 0
            : Math.round(Metrics.pt(Metrics.panelHeight + Metrics.panelBottomGap)));

        // Only the panel rectangle takes pointer events; everything else is
        // transparent to the windows underneath.
        DockSurface.setInputRegion(
            Math.round(panel.x),
            Math.round(panel.y),
            Math.round(panel.width),
            Math.round(panel.height));

        // Frosted glass behind the panel, following its capsule outline.
        DockSurface.setBlurRegion(panel.x, panel.y, panel.width, panel.height,
                                  Metrics.pt(Metrics.panelRadius));
    }

    Component.onCompleted: {
        syncSurface();
        if (Metrics.autoHide) {
            hideTimer.restart();
        }
    }

    Connections {
        target: panel
        function onWidthChanged() { window.syncSurface(); }
        function onXChanged() { window.syncSurface(); }
    }
}
