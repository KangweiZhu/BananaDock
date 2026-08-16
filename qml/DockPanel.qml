import QtQuick
import Qt.labs.folderlistmodel
import org.kde.taskmanager as TaskManager

import MacDock

/**
 * The dock panel itself: background plus a row of icons that magnify together.
 *
 * Magnification is not each icon scaling independently -- the whole row is laid
 * out again: first work out how wide every slot should be given its distance
 * from the cursor, then accumulate those widths to get each slot's position.
 * Neighbours get pushed aside, which is what macOS looks like.
 *
 * The row is not a single uniform model. macOS puts a separator and the Trash
 * after the applications, and those behave differently -- a separator is narrow
 * and must not magnify -- so the layout runs over a list of "slots" rather than
 * straight over the task model.
 */
Item {
    id: root

    /// Pointer x inside the panel, in resting-layout coordinates; -1 = not hovered.
    property real cursorRestX: -1

    /// Per-slot current width and x position, both filled in by relayout().
    property var itemWidths: []
    property var itemPositions: []
    /// Sum of all slot widths.
    property real contentWidth: 0

    readonly property real tilePx: Metrics.pt(Metrics.tileSize)
    readonly property real separatorPx: Metrics.pt(Metrics.separatorWidth)
    readonly property real panelW: contentWidth + Metrics.pt(Metrics.panelPaddingH) * 2
    readonly property real panelH: Metrics.pt(Metrics.panelHeight)

    implicitWidth: panelW
    implicitHeight: panelH

    TaskManager.TasksModel {
        id: tasksModel

        groupMode: TaskManager.TasksModel.GroupApplications
        sortMode: TaskManager.TasksModel.SortManual

        // Dock semantics: a launcher and its running windows share one tile.
        launchInPlace: true
        separateLaunchers: false

        onCountChanged: root.rebuildSlots()
    }

    /**
     * Watches the trash so the icon can switch between empty and full.
     *
     * FolderListModel already reports changes as they happen, so there is
     * nothing to poll.
     */
    FolderListModel {
        id: trashContents
        folder: "file://" + TrashPath
        showDirs: true
        showHidden: true
        showDotAndDotDot: false
    }

    readonly property bool trashFull: trashContents.count > 0

    /// Positions of the two non-application slots within the layout arrays;
    /// -1 when that slot is not present.
    property int separatorIndex: -1
    property int trashIndex: -1

    // -- Slot list ---------------------------------------------------------
    /// Describes the row: every application, then the separator, then Trash.
    property var slots: []

    function rebuildSlots() {
        let list = [];
        for (let i = 0; i < tasksModel.count; ++i) {
            list.push({ kind: "task", row: i });
        }
        if (Metrics.showTrash) {
            // Only worth a separator if there is something to separate from.
            if (list.length > 0) {
                list.push({ kind: "separator", row: -1 });
            }
            list.push({ kind: "trash", row: -1 });
        }
        root.separatorIndex = -1;
        root.trashIndex = -1;
        for (let i = 0; i < list.length; ++i) {
            if (list[i].kind === "separator") {
                root.separatorIndex = i;
            } else if (list[i].kind === "trash") {
                root.trashIndex = i;
            }
        }

        root.slots = list;
        root.relayout();
    }

    function restWidthOf(slot) {
        return slot.kind === "separator" ? root.separatorPx : root.tilePx;
    }

    /**
     * Works out how wide each slot should be, then where it sits.
     *
     * Distances are measured against each slot's RESTING centre rather than its
     * deformed position. That breaks the feedback loop where position affects
     * scale and scale affects position, so the layout stays stable. The cost is
     * that the icon under the cursor drifts slightly; calibrating that against
     * a reference is a separate step.
     */
    function relayout() {
        const list = root.slots;
        const magnifying = Metrics.magnificationEnabled && root.cursorRestX >= 0;
        const R = Metrics.magnificationRange * root.tilePx;
        const M = Metrics.maxScale;

        // Resting centres, needed before any scaling is applied. Slots are not
        // all the same width, so these have to be accumulated rather than
        // derived from the index.
        let restCentres = [];
        let acc = 0;
        for (let i = 0; i < list.length; ++i) {
            const w = root.restWidthOf(list[i]);
            restCentres.push(acc + w / 2);
            acc += w;
        }

        let widths = [];
        let positions = [];
        let total = 0;

        for (let i = 0; i < list.length; ++i) {
            const rest = root.restWidthOf(list[i]);
            let scale = 1.0;

            // Separators keep their width; only icons grow.
            if (magnifying && list[i].kind !== "separator") {
                const d = Math.abs(root.cursorRestX - restCentres[i]);
                if (d < R) {
                    // Raised cosine: peaks at M, decays smoothly to 1 at the
                    // edge with a continuous first derivative.
                    scale = 1.0 + (M - 1.0) * 0.5 * (Math.cos(Math.PI * (d / R)) + 1.0);
                }
            }

            const w = rest * scale;
            widths.push(w);
            positions.push(total);
            total += w;
        }

        root.itemWidths = widths;
        root.itemPositions = positions;
        root.contentWidth = total;
    }

    /**
     * Click behaviour, following macOS rather than the usual taskbar rules.
     *
     * Notably macOS never minimises an application from its dock icon -- that
     * is a Windows/Linux taskbar convention. Clicking the frontmost app instead
     * restores whatever windows of it are minimised.
     */
    function activateTask(row, item) {
        const idx = tasksModel.index(row, 0);

        // Not running yet: launching it produces a startup task, which is what
        // drives the bounce.
        if (item.IsLauncher === true) {
            tasksModel.requestActivate(idx);
            return;
        }

        if (item.IsActive === true) {
            root.restoreMinimised(idx, item);
            return;
        }

        root.raiseApplication(idx, item);
    }

    /// Brings every window of the application forward, not just the most
    /// recently used one -- on macOS the whole app comes to the front.
    function raiseApplication(idx, item) {
        if (item.IsGroupParent !== true) {
            tasksModel.requestActivate(idx);
            return;
        }

        // Activate back-to-front so the most recent window ends up focused.
        const count = tasksModel.rowCount(idx);
        for (let i = count - 1; i >= 0; --i) {
            tasksModel.requestActivate(tasksModel.index(i, 0, idx));
        }
    }

    function restoreMinimised(idx, item) {
        if (item.IsGroupParent !== true) {
            if (item.IsMinimized === true) {
                tasksModel.requestToggleMinimized(idx);
            }
            return;
        }

        const count = tasksModel.rowCount(idx);
        for (let i = 0; i < count; ++i) {
            const child = tasksModel.index(i, 0, idx);
            if (tasksModel.data(child, TaskManager.AbstractTasksModel.IsMinimized) === true) {
                tasksModel.requestToggleMinimized(child);
            }
        }
    }

    /// Hides every window of the application, as macOS's Hide does.
    function hideApplication(idx) {
        if (tasksModel.data(idx, TaskManager.AbstractTasksModel.IsGroupParent) !== true) {
            if (tasksModel.data(idx, TaskManager.AbstractTasksModel.IsMinimized) !== true) {
                tasksModel.requestToggleMinimized(idx);
            }
            return;
        }

        const count = tasksModel.rowCount(idx);
        for (let i = 0; i < count; ++i) {
            const child = tasksModel.index(i, 0, idx);
            if (tasksModel.data(child, TaskManager.AbstractTasksModel.IsMinimized) !== true) {
                tasksModel.requestToggleMinimized(child);
            }
        }
    }

    function openMenu(row, item) {
        contextMenu.taskIndex = row;
        // Anchor to the icon and let the menu open upwards; the dock sits at
        // the bottom of the screen so there is no room below it.
        const pos = item.mapToItem(root, item.width / 2, 0);
        contextMenu.x = pos.x - contextMenu.width / 2;
        contextMenu.y = -contextMenu.height;
        contextMenu.open();
    }

    DockMenu {
        id: contextMenu
        tasksModel: tasksModel

        onShowAllWindowsRequested: root.raiseApplication(taskIdx,
            { IsGroupParent: tasksModel.data(taskIdx, TaskManager.AbstractTasksModel.IsGroupParent) })
        onHideRequested: root.hideApplication(taskIdx)
    }

    onCursorRestXChanged: relayout()
    Component.onCompleted: rebuildSlots()

    Connections {
        target: Metrics
        function onShowTrashChanged() { root.rebuildSlots(); }
    }

    // -- Panel background --------------------------------------------------
    // The real blur is composited by KWin behind the surface via
    // ext-background-effect-v1. This is only the translucent tint and the
    // highlight stroke that sit on top of it.
    Rectangle {
        id: background

        anchors.fill: parent
        radius: Metrics.pt(Metrics.panelRadius)
        color: Metrics.panelTint
        border.color: Metrics.panelBorderColor
        border.width: Metrics.pt(Metrics.panelBorderWidth)
    }

    // -- Icon row ----------------------------------------------------------
    // Applications stay on a model-bound Repeater so role changes (icon, active
    // state, startup) keep flowing through. Binding to tasksModel.data() from a
    // plain array model would look equivalent but would never re-evaluate on
    // dataChanged, leaving stale icons.
    Item {
        id: content

        x: Metrics.pt(Metrics.panelPaddingH)
        width: root.contentWidth
        anchors.top: parent.top
        anchors.bottom: parent.bottom

        Repeater {
            id: repeater
            model: tasksModel

            delegate: DockItem {
                id: taskItem

                required property int index
                required property var model

                iconSource: model.decoration
                isRunning: model.IsWindow === true || model.IsGroupParent === true
                isLauncher: model.IsLauncher === true
                isStarting: model.IsStartup === true

                tilePx: root.itemWidths[index] !== undefined
                        ? root.itemWidths[index]
                        : root.tilePx
                x: root.itemPositions[index] !== undefined ? root.itemPositions[index] : 0

                Behavior on tilePx {
                    NumberAnimation {
                        duration: Metrics.magnifyDuration
                        easing.type: Easing.OutQuad
                    }
                }
                Behavior on x {
                    NumberAnimation {
                        duration: Metrics.magnifyDuration
                        easing.type: Easing.OutQuad
                    }
                }

                onClicked: root.activateTask(index, model)
                onRightClicked: root.openMenu(index, taskItem)
            }
        }

        // -- Separator ------------------------------------------------------
        Item {
            id: separator

            visible: root.separatorIndex >= 0
            width: root.itemWidths[root.separatorIndex] !== undefined
                   ? root.itemWidths[root.separatorIndex]
                   : root.separatorPx
            x: root.itemPositions[root.separatorIndex] !== undefined
               ? root.itemPositions[root.separatorIndex]
               : 0
            anchors.top: parent.top
            anchors.bottom: parent.bottom

            Behavior on x {
                NumberAnimation {
                    duration: Metrics.magnifyDuration
                    easing.type: Easing.OutQuad
                }
            }

            Rectangle {
                anchors.centerIn: parent
                width: Metrics.pt(Metrics.separatorLineWidth)
                height: parent.height - Metrics.pt(Metrics.separatorInset) * 2
                radius: width / 2
                color: Metrics.separatorColor
            }
        }

        // -- Trash ----------------------------------------------------------
        DockItem {
            id: trash

            visible: root.trashIndex >= 0

            iconSource: root.trashFull ? "user-trash-full" : "user-trash"
            isRunning: false
            isLauncher: true
            isStarting: false

            tilePx: root.itemWidths[root.trashIndex] !== undefined
                    ? root.itemWidths[root.trashIndex]
                    : root.tilePx
            x: root.itemPositions[root.trashIndex] !== undefined
               ? root.itemPositions[root.trashIndex]
               : 0

            Behavior on tilePx {
                NumberAnimation {
                    duration: Metrics.magnifyDuration
                    easing.type: Easing.OutQuad
                }
            }
            Behavior on x {
                NumberAnimation {
                    duration: Metrics.magnifyDuration
                    easing.type: Easing.OutQuad
                }
            }

            onClicked: Qt.openUrlExternally("trash:/")
        }
    }
}
