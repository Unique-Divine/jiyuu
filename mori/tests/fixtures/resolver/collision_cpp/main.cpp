struct Alpha { int render(){return 1;} };
struct Bravo { int render(){return 2;} };
int occurrence_probe(){
 Alpha alpha; Bravo bravo;
 /*xxxxxxxxxxxxxxxxxxxxx*/ alpha . render ( );
#define HIDDEN_RENDER(x) x.render()
 return HIDDEN_RENDER(bravo);
}
