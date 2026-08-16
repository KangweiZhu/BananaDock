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

    /// Current tile PITCH in px, already scaled. Set by DockPanel; grows with
    /// magnification. The icon artwork is a fixed fraction of it.
    property real tilePx: Metrics.pt(Metrics.tileSize)

    readonly property real iconPx: tilePx * Metrics.iconSizeRatio

    signal clicked()

    width: tilePx
    height: parent ? parent.height : tilePx

    Kirigami.Icon {
        id: icon

        source: root.iconSource
        smooth: true

        width: root.iconPx
        height: root.iconPx

        // Icons are bottom-aligned and grow upwards when magnified, as on macOS.
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Metrics.pt(Metrics.iconBottomMargin)
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
        anchors.bottomMargin: Metrics.pt(Metrics.dotBottomMargin)
    }

    TapHandler {
        onTapped: root.clicked()
    }
}
