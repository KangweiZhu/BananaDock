import QtQuick
import org.kde.kirigami as Kirigami

import MacDock

/**
 * A single dock icon.
 *
 * Size and horizontal position are computed by DockPanel and passed in.
 * Magnification is a whole-row layout problem, not each icon scaling itself --
 * that distinction is what makes the macOS feel work.
 */
Item {
    id: root

    required property var iconSource
    required property bool isRunning
    required property bool isLauncher

    /// Current edge length, already multiplied by scaleFactor. Set by DockPanel.
    property real tilePx: Metrics.pt(Metrics.tileSize)

    signal clicked()

    width: tilePx
    height: parent ? parent.height : tilePx

    Kirigami.Icon {
        id: icon

        source: root.iconSource
        smooth: true

        width: root.tilePx
        height: root.tilePx

        // Icons are bottom-aligned and grow upwards when magnified, as on macOS.
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Metrics.pt(Metrics.panelPaddingV + Metrics.dotBottomMargin * 2)
    }

    // Running-application indicator
    Rectangle {
        visible: root.isRunning
        width: Metrics.pt(Metrics.dotSize)
        height: width
        radius: width / 2
        color: Metrics.dotColor

        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Metrics.pt(Metrics.panelPaddingV + Metrics.dotBottomMargin)
    }

    TapHandler {
        onTapped: root.clicked()
    }
}
