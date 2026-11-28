package Base;
sub greet { return 1; }
package Child;
our @ISA = ('Base');
sub new { my $class = shift; return bless {}, $class; }
package main;
sub inherited_occurrence_probe {
    my $child = Child->new();
    $child->greet();
    $child->greet();
}
