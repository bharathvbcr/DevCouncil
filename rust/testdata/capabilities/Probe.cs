using Example.Helpers;

namespace Probe
{
    public class Widget : BaseWidget, IRenderable
    {
        private string name;

        public string Render()
        {
            return Helper.Help(this.name);
        }

        public static void Main()
        {
            var w = new Widget();
            w.Render();
        }
    }
}
