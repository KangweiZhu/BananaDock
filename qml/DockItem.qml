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
    /// True while the application is starting up; drives the launch bounce.
    required property bool isStarting

    /// Current tile PITCH in px, already scaled. Set by DockPanel; grows with
    /// magnification. The icon artwork is a fixed fraction of it.
    property real tilePx: Metrics.pt(Metrics.tileSize)

    readonly property real iconPx: tilePx * Metrics.iconSizeRatio

    /// Height of the launch bounce above the resting position, in px.
    property real bounceOffset: 0

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
        anchors.bottomMargin: Metrics.pt(Metrics.iconBottomMargin) + root.bounceOffset
    }

    /**
     * Launch bounce.
     *
     * The rise and the fall get opposite easings rather than one symmetric
     * curve: a real jump decelerates on the way up and accelerates on the way
     * down. Easing both halves the same way reads as floaty.
     */
    SequentialAnimation {
        running: root.isStarting
        loops: Animation.Infinite
        // Let the current hop land instead of freezing the icon mid-air when
        // the app finishes starting.
        alwaysRunToEnd: true

        NumberAnimation {
            target: root
            property: "bounceOffset"
            to: Metrics.pt(Metrics.bounceHeight)
            duration: Metrics.bounceDuration * 0.45
            easing.type: Easing.OutQuad
        }
        NumberAnimation {
            target: root
            property: "bounceOffset"
            to: 0
            duration: Metrics.bounceDuration * 0.55
            easing.type: Easing.InQuad
        }
        PauseAnimation {
            duration: Metrics.bounceRestDuration
        }

        onStopped: root.bounceOffset = 0
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
