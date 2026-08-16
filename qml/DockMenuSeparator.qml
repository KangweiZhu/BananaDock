import QtQuick
import QtQuick.Controls as QQC

import MacDock

/**
 * A menu separator that collapses to zero height when hidden.
 *
 * A plain `visible: false` still leaves the item's height in the Menu's
 * layout, which shows up as an unexplained gap.
 */
QQC.MenuSeparator {
    id: control

    /// Whether this separator takes part in the menu at all.
    property bool shown: true

    visible: shown
    height: shown ? implicitHeight : 0

    topPadding: Metrics.pt(Metrics.menuSeparatorMargin)
    bottomPadding: Metrics.pt(Metrics.menuSeparatorMargin)
    leftPadding: Metrics.pt(Metrics.menuHighlightInset)
    rightPadding: Metrics.pt(Metrics.menuHighlightInset)

    contentItem: Rectangle {
        implicitHeight: 1
        color: Metrics.menuSeparatorColor
    }
}
