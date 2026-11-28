package Widget;
sub render { return 1; }
package main;
sub occurrence_probe {
    Widget->render();
    Widget->render();
}
